import { Effect, Layer, Option } from "effect";
import * as Fs from "@effect/platform/FileSystem";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { ClaudeCodeRepository } from "sepia-claude";
import { ClineIndex, ClineRepository } from "sepia-cline";
import { ClineStore } from "sepia-convert";
import { CursorRepository } from "sepia-cursor";
import { openSessionsDb, SqliteStorage } from "sepia-devin";
import type { Session } from "sepia-core";
import { ControlError } from "sepia-session-control";
import type { SessionRewinder } from "sepia-session-control";

/**
 * Per-agent `SessionRewinder`s the server injects into the control plane.
 * Each store truncates its own way:
 *
 * - `devin` — in-place `message_nodes` row delete on `SEPIA_DB` (the
 *   store the merged repo holds read-only; a second, writable connection
 *   deletes the suffix in one transaction without rewriting survivors).
 * - `cline` — slice the session's `<id>.messages.json` at the boundary
 *   the plan's `clineMessageIndex` tags imply; refused while the index
 *   row shows a live owner (status running/idle/pending with a live pid).
 * - `claude` — rewrite the `.jsonl` ending at the entry that produced
 *   the last kept node (uuid → line map; prefix rewrite, atomic rename).
 * - `cursor` — `save` the truncated IR: the content-addressed store gets
 *   a fresh checkpoint root whose message list is the kept nodes — blob
 *   inserts are additive, so the older DAG stays intact underneath.
 */

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

/** Store failures become `internal`; a `ControlError` keeps its code (e.g. `locked`). */
const asRewindWrite = (effect: Effect.Effect<unknown, unknown>): Effect.Effect<void, unknown> =>
  effect.pipe(
    Effect.asVoid,
    Effect.mapError((cause) =>
      cause instanceof ControlError
        ? cause
        : new ControlError({
            code: "internal",
            message: errorMessage(cause),
            cause,
          }),
    ),
  );

export interface RewinderPaths {
  readonly dbPath: string;
  readonly clineDir: string;
  readonly claudeDir: string;
  readonly cursorDir: string;
}

export const makeRewinders = (paths: RewinderPaths): Record<string, SessionRewinder> => ({
  devin: {
    truncate: (session: Session, plan, truncated) =>
      asRewindWrite(
        SqliteStorage.truncateSessionNodes(paths.dbPath, session.id, {
          removedNodeIds: plan.removed.map((node) => node.nodeId),
          removedToolCallIds: plan.removedToolCallIds,
          lastActivityAt: truncated.lastActivityAt,
          mainChainId: truncated.mainChainId,
        }),
      ),
  },

  cline: {
    truncate: (session: Session, plan) =>
      asRewindWrite(
        Effect.gen(function* () {
          const fs = yield* Fs.FileSystem;
          // The index row's status/pid is what `cline` itself trusts — a
          // live owner would replay the pre-rewind tail from memory.
          const row = yield* ClineStore.indexRow(
            openSessionsDb,
            fs,
            `${paths.clineDir}/db/sessions.db`,
            session.id,
          );
          if (
            Option.isSome(row) &&
            ClineIndex.isActiveRow(row.value, ClineIndex.isPidAlive(row.value.pid))
          ) {
            return yield* Effect.fail(
              new ControlError({
                code: "locked",
                message: `Session ${session.id} is held by a live Cline process (pid ${row.value.pid})`,
                cause: null,
              }),
            );
          }
          yield* ClineRepository.truncateClineSession(
            { dataDir: paths.clineDir },
            session.id,
            plan.kept,
            plan.removed,
          );
        }).pipe(Effect.provide(fsLayer)),
      ),
  },

  claude: {
    truncate: (session: Session, plan) =>
      asRewindWrite(
        ClaudeCodeRepository.truncateClaudeTranscript(
          { projectsDir: `${paths.claudeDir}/projects` },
          session.id,
          plan.kept,
          plan.removed,
        ),
      ),
  },

  cursor: {
    truncate: (_session: Session, _plan, truncated) =>
      asRewindWrite(
        CursorRepository.makeCursorSessionRepository({ cursorDir: paths.cursorDir }).save(
          truncated,
        ),
      ),
  },
});
