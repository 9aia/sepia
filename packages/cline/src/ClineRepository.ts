import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import * as Cline from "./Cline.js";
import { MessageNode, Session, StorageError } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";

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

/** The `clineMessageIndex` a reader-tagged node carries; anything else is unmappable. */
const sourceIndexOf = (node: MessageNode): number | undefined => {
  const meta = node.metadata;
  if (meta === null || typeof meta !== "object") return undefined;
  const index = (meta as Record<string, unknown>).clineMessageIndex;
  return typeof index === "number" && Number.isInteger(index) && index >= 0 ? index : undefined;
};

const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/**
 * In-place conversation rewind for a Cline session: slice the transcript's
 * `messages` array just past the last kept node's source entry.
 *
 * Each node emitted by `Cline.fromDirectory` records its source position
 * in `metadata.clineMessageIndex`, so the cut lands on whole messages —
 * nodes sharing one entry (assistant twins, a multi-result turn) survive
 * or drop together even when the requested boundary lands between them.
 * Every other top-level field of the messages file (`version`, `agent`,
 * `origin`, fields the reader never modeled) is preserved verbatim and
 * the manifest is never touched — strictly less invasive than a
 * `ClineStore.install` rebuild, which regenerates the whole pair from IR.
 *
 * Refuses when the resolved `messages_path` escapes the data dir or the
 * file provably belongs to another session (a subagent manifest pointing
 * at the parent's transcript would truncate the parent's history).
 */
export const truncateClineSession = (
  options: ClineRepositoryOptions,
  sessionId: string,
  kept: ReadonlyArray<MessageNode>,
  removed: ReadonlyArray<MessageNode>,
): Effect.Effect<{ readonly removedMessages: number }, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const path = yield* Path.Path;
    const dir = path.join(options.dataDir, "sessions", sessionId);

    // Resolve the transcript the way `Cline.fromDirectory` does: manifest's
    // `messages_path`, else the `<id>.messages.json` sibling.
    const entries = yield* fs.readDirectory(dir);
    const metaName = entries.find(
      (name) =>
        name.endsWith(".json") &&
        !name.endsWith(".messages.json") &&
        !name.includes(".compaction."),
    );
    if (metaName === undefined) {
      return yield* Effect.fail(
        new StorageError({ message: `No session metadata json found in ${dir}` }),
      );
    }
    const base = metaName.replace(/\.json$/, "");
    const metaRaw = yield* fs.readFileString(path.join(dir, metaName));
    let meta: unknown;
    try {
      meta = JSON.parse(metaRaw);
    } catch {
      return yield* Effect.fail(
        new StorageError({ message: `Cline manifest is not valid JSON: ${metaName}` }),
      );
    }
    const declaredPath =
      isObject(meta) && typeof meta.messages_path === "string" && meta.messages_path !== ""
        ? meta.messages_path
        : undefined;
    const resolved = path.resolve(
      declaredPath === undefined ? path.join(dir, `${base}.messages.json`) : declaredPath,
    );
    const root = path.resolve(options.dataDir);
    if (resolved !== root && !resolved.startsWith(root + path.sep)) {
      return yield* Effect.fail(
        new StorageError({
          message: `Cline messages_path escapes the data dir: ${declaredPath ?? ""}`,
        }),
      );
    }

    const raw = yield* fs.readFileString(resolved);
    let data: unknown;
    try {
      data = JSON.parse(raw);
    } catch {
      return yield* Effect.fail(
        new StorageError({ message: `Cline transcript is not valid JSON: ${resolved}` }),
      );
    }
    if (!isObject(data) || !Array.isArray(data.messages)) {
      return yield* Effect.fail(
        new StorageError({ message: `Cline transcript carries no message array: ${resolved}` }),
      );
    }
    // The file must belong to this session — a manifest pointing at a
    // transcript that names another id would cut that session's history.
    const fileSessionId =
      typeof data.sessionId === "string"
        ? data.sessionId
        : isObject(data.origin) && typeof data.origin.sessionId === "string"
          ? data.origin.sessionId
          : undefined;
    if (fileSessionId !== undefined && fileSessionId !== sessionId) {
      return yield* Effect.fail(
        new StorageError({
          message: `Cline transcript belongs to ${fileSessionId}, not ${sessionId}`,
        }),
      );
    }
    if (fileSessionId === undefined && path.basename(resolved) !== `${sessionId}.messages.json`) {
      return yield* Effect.fail(
        new StorageError({
          message: `Cannot prove ${resolved} is ${sessionId}'s transcript — refusing to truncate`,
        }),
      );
    }

    const messages = data.messages as ReadonlyArray<unknown>;
    const keptIndices = kept.flatMap((node) => {
      const index = sourceIndexOf(node);
      return index === undefined ? [] : [index];
    });
    for (const node of removed) {
      if (sourceIndexOf(node) === undefined) {
        return yield* Effect.fail(
          new StorageError({
            message: `Node ${node.nodeId} has no recorded transcript entry — cannot truncate`,
          }),
        );
      }
    }
    const cutIndex = keptIndices.length === 0 ? 0 : Math.max(...keptIndices) + 1;
    if (cutIndex >= messages.length) return { removedMessages: 0 };

    const out = {
      ...data,
      updated_at: new Date().toISOString(),
      messages: messages.slice(0, cutIndex),
    };
    // tmp + rename: a crash mid-write must not leave a torn transcript.
    const tmp = `${resolved}.tmp-${process.pid}`;
    yield* fs.writeFileString(tmp, JSON.stringify(out, null, 2));
    yield* fs.rename(tmp, resolved);
    return { removedMessages: messages.length - cutIndex };
  }).pipe(
    Effect.provide(fsLayer),
    Effect.mapError((error) =>
      error instanceof StorageError
        ? error
        : storageError("Failed to truncate cline session")(error),
    ),
  );
