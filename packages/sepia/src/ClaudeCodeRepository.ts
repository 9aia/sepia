import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import * as ClaudeCode from "./ClaudeCode.js";
import { Session, StorageError } from "./Domain.js";
import type { SessionRepositoryService } from "./Storage.js";

export interface ClaudeCodeRepositoryOptions {
  /** Claude Code projects root, usually `~/.claude/projects`. */
  readonly projectsDir: string;
}

interface TranscriptFile {
  readonly filePath: string;
  readonly id: string;
  readonly fallbackCwd: string;
  readonly parentSessionId: string | undefined;
}

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const storageError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

const JSONL = ".jsonl";

/**
 * Read-only SessionRepository over Claude Code's on-disk transcripts. The
 * store has no manifest, so `list()` reads each `.jsonl` and summarizes it
 * without building nodes; `getById` parses the full transcript.
 *
 * Layout handled: `<projects>/<slug>/<uuid>.jsonl` main sessions plus
 * sub-agent transcripts in `<uuid>/subagents/agent-*.jsonl` (current) and
 * `agent-*.jsonl` at the project root (legacy).
 */
export const makeClaudeCodeSessionRepository = (
  options: ClaudeCodeRepositoryOptions,
): SessionRepositoryService => {
  /** Every transcript file under the projects root, with its provenance. */
  const transcriptFiles = (): Effect.Effect<
    ReadonlyArray<TranscriptFile>,
    unknown,
    Fs.FileSystem | Path.Path
  > =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const path = yield* Path.Path;
      const exists = yield* fs.exists(options.projectsDir).pipe(Effect.orElseSucceed(() => false));
      if (!exists) return [] as ReadonlyArray<TranscriptFile>;
      const dirs = yield* fs.readDirectory(options.projectsDir);
      const files: Array<TranscriptFile> = [];
      for (const dirName of dirs) {
        const dir = path.join(options.projectsDir, dirName);
        const info = yield* fs.stat(dir).pipe(Effect.option);
        if (Option.isNone(info) || info.value.type !== "Directory") continue;
        const fallbackCwd = ClaudeCode.decodeProjectDir(dirName);
        const entries = yield* fs
          .readDirectory(dir)
          .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>));
        for (const entry of entries) {
          if (entry.endsWith(JSONL)) {
            files.push({
              filePath: path.join(dir, entry),
              id: entry.slice(0, -JSONL.length),
              fallbackCwd,
              parentSessionId: undefined,
            });
            continue;
          }
          // A `<uuid>/` directory may hold `subagents/agent-*.jsonl`.
          const subDir = path.join(dir, entry, "subagents");
          const hasSubDir = yield* fs.exists(subDir).pipe(Effect.orElseSucceed(() => false));
          if (!hasSubDir) continue;
          const subs = yield* fs
            .readDirectory(subDir)
            .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>));
          for (const sub of subs) {
            if (!sub.endsWith(JSONL)) continue;
            files.push({
              filePath: path.join(subDir, sub),
              id: sub.slice(0, -JSONL.length),
              fallbackCwd,
              parentSessionId: entry,
            });
          }
        }
      }
      return files;
    });

  return {
    list: () =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        const sessions: Array<Session> = [];
        for (const file of yield* transcriptFiles()) {
          const raw = yield* fs.readFileString(file.filePath).pipe(Effect.orElseSucceed(() => ""));
          if (raw === "") continue;
          sessions.push(
            ClaudeCode.summarizeJsonl(raw, {
              id: file.id,
              fallbackCwd: file.fallbackCwd,
              parentSessionId: file.parentSessionId,
            }),
          );
        }
        return sessions.sort((a, b) => b.lastActivityAt - a.lastActivityAt);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to list claude sessions")),
      ),

    getById: (id) =>
      Effect.gen(function* () {
        const file = (yield* transcriptFiles()).find((candidate) => candidate.id === id);
        if (file === undefined) return Option.none<Session>();
        // `fromFile` re-derives id, decoded cwd and subagent parentage from
        // the path — the same values the scan computed.
        const session = yield* ClaudeCode.fromFile(file.filePath);
        return Option.some(session);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.catchAll((error: unknown) => {
          const message = error instanceof Error ? error.message : String(error);
          return /not found/i.test(message)
            ? Effect.succeed(Option.none<Session>())
            : Effect.fail(
                new StorageError({ message: `Failed to read claude session: ${message}` }),
              );
        }),
      ),

    hasSession: (id) =>
      transcriptFiles().pipe(
        Effect.map((files) => files.some((file) => file.id === id)),
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to check claude session")),
      ),

    save: () => Effect.fail(new StorageError({ message: "Claude repository is read-only" })),
    delete: () => Effect.fail(new StorageError({ message: "Claude repository is read-only" })),
  };
};
