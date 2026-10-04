import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import * as Cline from "./Cline.js";
import { Session, StorageError } from "./Domain.js";
import type { SessionRepositoryService } from "./Storage.js";

export interface ClineRepositoryOptions {
  /** Cline data directory, usually `~/.cline/data`. */
  readonly dataDir: string;
}

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const storageError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

const toSeconds = (value: unknown): number | undefined =>
  typeof value === "string" && !Number.isNaN(new Date(value).getTime())
    ? Math.floor(new Date(value).getTime() / 1000)
    : undefined;

const titleOf = (meta: Record<string, unknown>, fallback: string): string => {
  const metadata = meta.metadata;
  if (metadata && typeof metadata === "object") {
    const title = (metadata as Record<string, unknown>).title;
    if (typeof title === "string" && title.trim() !== "") return title.trim();
  }
  if (typeof meta.prompt === "string") {
    const text = meta.prompt
      .replace(/<[^>]+>/g, " ")
      .replace(/\s+/g, " ")
      .trim();
    if (text !== "") return text.slice(0, 80);
  }
  return fallback;
};

/** Manifest-only Session for `list()`; `getById` loads the real transcript. */
const manifestToSession = (raw: string, fallbackId: string): Session | null => {
  try {
    const meta = JSON.parse(raw) as Record<string, unknown>;
    const id = typeof meta.session_id === "string" ? meta.session_id : fallbackId;
    const createdAt = toSeconds(meta.started_at) ?? Math.floor(Date.now() / 1000);
    const subagent = Cline.clineSubagentInfo(id);
    return Session.make({
      id,
      title: titleOf(meta, id),
      workingDirectory: typeof meta.cwd === "string" ? meta.cwd : "/",
      backendType: "cline",
      model: typeof meta.model === "string" ? meta.model.replace(/^cline-pass\//, "") : "unknown",
      createdAt,
      lastActivityAt: toSeconds(meta.ended_at) ?? createdAt,
      mainChainId: 0,
      parentSessionId: Option.fromNullable(subagent?.parentSessionId),
      agentId: Option.fromNullable(subagent?.agentId),
      checkpoints: Cline.checkpointsFromManifest(meta),
      metadata: {},
    });
  } catch {
    return null;
  }
};

const isManifest = (name: string): boolean =>
  name.endsWith(".json") && !name.endsWith(".messages.json") && !name.includes(".compaction.");

/**
 * Read-only SessionRepository over Cline's on-disk session dirs. The control
 * plane only reads through it; writes go through `ClineStore.install` or the
 * agent itself, never this path.
 */
export const makeClineSessionRepository = (
  options: ClineRepositoryOptions,
): SessionRepositoryService => {
  const sessionsDir = () => `${options.dataDir}/sessions`;

  return {
    list: () =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        const path = yield* Path.Path;
        const root = sessionsDir();
        const exists = yield* fs.exists(root).pipe(Effect.orElseSucceed(() => false));
        if (!exists) return [] as ReadonlyArray<Session>;
        const entries = yield* fs.readDirectory(root);
        const sessions: Array<Session> = [];
        for (const entry of entries) {
          const dir = path.join(root, entry);
          const info = yield* fs.stat(dir).pipe(Effect.option);
          if (Option.isNone(info) || info.value.type !== "Directory") continue;
          const files = yield* fs
            .readDirectory(dir)
            .pipe(Effect.orElseSucceed(() => [] as ReadonlyArray<string>));
          const metaName = files.find(isManifest);
          if (metaName === undefined) continue;
          const raw = yield* fs
            .readFileString(path.join(dir, metaName))
            .pipe(Effect.orElseSucceed(() => ""));
          const session = manifestToSession(raw, entry);
          if (session !== null) sessions.push(session);
        }
        return sessions.sort((a, b) => b.lastActivityAt - a.lastActivityAt);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to list cline sessions")),
      ),

    getById: (id) =>
      Cline.fromDirectory(`${sessionsDir()}/${id}`).pipe(
        Effect.map((session) =>
          Session.make({
            id: session.id,
            title: session.title,
            workingDirectory: session.workingDirectory,
            backendType: "cline",
            agentMode: session.agentMode,
            model: session.model,
            createdAt: session.createdAt,
            lastActivityAt: session.lastActivityAt,
            mainChainId: session.mainChainId,
            shellLastSeenIndex: session.shellLastSeenIndex,
            cogsJson: session.cogsJson,
            workspaceDirs: session.workspaceDirs,
            hidden: session.hidden,
            parentSessionId: session.parentSessionId,
            agentId: session.agentId,
            checkpoints: session.checkpoints,
            metadata: session.metadata,
            nodes: session.nodes,
            promptHistory: session.promptHistory,
          }),
        ),
        Effect.map(Option.some),
        Effect.provide(fsLayer),
        Effect.catchAll((error) =>
          /not found/i.test(error.message)
            ? Effect.succeed(Option.none<Session>())
            : Effect.fail(new StorageError({ message: error.message })),
        ),
      ),

    hasSession: (id) =>
      Effect.gen(function* () {
        const fs = yield* Fs.FileSystem;
        return yield* fs.exists(`${sessionsDir()}/${id}`).pipe(Effect.orElseSucceed(() => false));
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to check cline session")),
      ),

    save: () => Effect.fail(new StorageError({ message: "Cline repository is read-only" })),
    delete: () => Effect.fail(new StorageError({ message: "Cline repository is read-only" })),
  };
};
