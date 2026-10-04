import { Effect, Layer, Option } from "effect";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import * as ClaudeCode from "./ClaudeCode.js";
import * as Cursor from "./Cursor.js";
import { Session, StorageError } from "./Domain.js";
import type { SessionRepositoryService } from "./Storage.js";

/** Minimal sqlite surface a Cursor `store.db` needs — injectable for tests. */
export interface CursorStoreDb {
  readonly all: (
    sql: string,
    params?: ReadonlyArray<unknown>,
  ) => ReadonlyArray<Record<string, unknown>>;
  readonly close: () => void;
}

export type OpenStoreDb = (path: string) => Effect.Effect<CursorStoreDb, StorageError>;

export interface CursorRepositoryOptions {
  /** Cursor data dir, usually `~/.cursor`. */
  readonly cursorDir: string;
  /** Opens a `store.db`; defaults to `bun:sqlite` loaded lazily. */
  readonly openStoreDb?: OpenStoreDb;
}

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const storageError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

/** The slice of `bun:sqlite`'s Database a store.db read needs. */
interface BunSqliteModule {
  readonly Database: new (
    path: string,
    options?: { readonly readonly?: boolean },
  ) => {
    readonly query: (sql: string) => {
      readonly all: (...params: ReadonlyArray<unknown>) => unknown[];
    };
    readonly close: () => void;
  };
}

// Indirection keeps TypeScript/vitest from resolving `bun:sqlite` at module
// load — packages that don't map bun types stay clean, and Node test
// runners only hit this when a real store.db is opened.
const BUN_SQLITE_MODULE: string = "bun:sqlite";

/**
 * `bun:sqlite` resolves lazily so this module stays importable under Node
 * (vitest); the import only runs when a real store.db is opened.
 */
const defaultOpenStoreDb: OpenStoreDb = (dbPath) =>
  Effect.tryPromise({
    try: async () => {
      const { Database } = (await import(BUN_SQLITE_MODULE)) as BunSqliteModule;
      const sqlite = new Database(dbPath, { readonly: true });
      return {
        all: (sql, params = []) =>
          sqlite.query(sql).all(...params) as ReadonlyArray<Record<string, unknown>>,
        close: () => sqlite.close(),
      };
    },
    catch: (cause) =>
      new StorageError({
        message: `Failed to open cursor store ${dbPath}: ${cause instanceof Error ? cause.message : String(cause)}`,
      }),
  });

interface ChatEntry {
  readonly kind: "chat";
  readonly dir: string;
  readonly id: string;
  readonly workspaceHash: string;
  readonly hasStore: boolean;
  readonly hasMeta: boolean;
}

interface TranscriptEntry {
  readonly kind: "transcript";
  readonly filePath: string;
  readonly id: string;
  readonly projectSlug: string;
  readonly parentSessionId: string | undefined;
}

type StoreEntry = ChatEntry | TranscriptEntry;

const JSONL = ".jsonl";

/**
 * Read-only SessionRepository over Cursor's two on-disk stores:
 *
 * - `chats/<workspace-hash>/<chat-id>/` — the agent CLI's content-addressed
 *   `store.db` plus `meta.json`/`prompt_history.json` sidecars. Canonical
 *   transcript: ordered JSON message blobs listed by the latest checkpoint.
 * - `projects/<slug>/agent-transcripts/<chat-id>/<chat-id>.jsonl` — the
 *   lossy text projection kept for every chat, including ones whose
 *   `store.db` was pruned. A chat covered by `chats/` wins on the shared id;
 *   `<chat-id>/subagents/<id>.jsonl` files link to their parent chat.
 *
 * Both trees degrade to empty when the dir is missing, matching the other
 * overlay repositories.
 */
export const makeCursorSessionRepository = (
  options: CursorRepositoryOptions,
): SessionRepositoryService => {
  const openStoreDb = options.openStoreDb ?? defaultOpenStoreDb;
  const chatsDir = () => `${options.cursorDir}/chats`;
  const projectsDir = () => `${options.cursorDir}/projects`;

  const listDir = (dir: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const exists = yield* fs.exists(dir).pipe(Effect.orElseSucceed(() => false));
      if (!exists) return [] as ReadonlyArray<string>;
      return yield* fs.readDirectory(dir).pipe(Effect.orElseSucceed(() => []));
    });

  /** Every chat dir and transcript file under the cursor root, with provenance. */
  const scanEntries = (): Effect.Effect<
    ReadonlyArray<StoreEntry>,
    unknown,
    Fs.FileSystem | Path.Path
  > =>
    Effect.gen(function* () {
      const path = yield* Path.Path;
      const fs = yield* Fs.FileSystem;
      const entries: Array<StoreEntry> = [];

      for (const wsHash of yield* listDir(chatsDir())) {
        const wsDir = path.join(chatsDir(), wsHash);
        const wsInfo = yield* fs.stat(wsDir).pipe(Effect.option);
        if (Option.isNone(wsInfo) || wsInfo.value.type !== "Directory") continue;
        for (const chatId of yield* listDir(wsDir)) {
          const chatDir = path.join(wsDir, chatId);
          const chatInfo = yield* fs.stat(chatDir).pipe(Effect.option);
          if (Option.isNone(chatInfo) || chatInfo.value.type !== "Directory") continue;
          const files = yield* listDir(chatDir);
          entries.push({
            kind: "chat",
            dir: chatDir,
            id: chatId,
            workspaceHash: wsHash,
            hasStore: files.includes("store.db"),
            hasMeta: files.includes("meta.json"),
          });
        }
      }

      for (const slug of yield* listDir(projectsDir())) {
        const transcriptsDir = path.join(projectsDir(), slug, "agent-transcripts");
        for (const chatId of yield* listDir(transcriptsDir)) {
          const chatDir = path.join(transcriptsDir, chatId);
          const chatInfo = yield* fs.stat(chatDir).pipe(Effect.option);
          if (Option.isNone(chatInfo) || chatInfo.value.type !== "Directory") continue;
          const main = path.join(chatDir, `${chatId}${JSONL}`);
          if (yield* fs.exists(main).pipe(Effect.orElseSucceed(() => false))) {
            entries.push({
              kind: "transcript",
              filePath: main,
              id: chatId,
              projectSlug: slug,
              parentSessionId: undefined,
            });
          }
          const subDir = path.join(chatDir, "subagents");
          for (const sub of yield* listDir(subDir)) {
            if (!sub.endsWith(JSONL)) continue;
            entries.push({
              kind: "transcript",
              filePath: path.join(subDir, sub),
              id: sub.slice(0, -JSONL.length),
              projectSlug: slug,
              parentSessionId: chatId,
            });
          }
        }
      }
      return entries;
    });

  const readFileOrEmpty = (filePath: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      return yield* fs.readFileString(filePath).pipe(Effect.orElseSucceed(() => ""));
    });

  const mtimeMs = (filePath: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const info = yield* fs.stat(filePath).pipe(Effect.option);
      return Option.flatMap(info, (i) => i.mtime).pipe(
        Option.map((d) => d.getTime()),
        Option.getOrUndefined,
      );
    });

  /** meta['0'] plus the root checkpoint's workspace, from an open store. */
  const storeSummary = (chatDir: string) =>
    Effect.gen(function* () {
      const db = yield* openStoreDb(`${chatDir}/store.db`);
      try {
        const metaRow = db.all("select value from meta where key = '0'")[0];
        const meta = Cursor.parseStoreMeta(metaRow?.value);
        let workspace: string | undefined;
        const rootId = meta?.latestRootBlobId;
        if (rootId !== undefined && rootId !== "") {
          const row = db.all("select data from blobs where id = ?", [rootId])[0];
          const data = row?.data;
          if (data instanceof Uint8Array) {
            workspace = Cursor.workspaceFromUri(Cursor.decodeCheckpoint(data)?.workspace);
          }
        }
        return { meta, workspace };
      } finally {
        db.close();
      }
    });

  const chatSummary = (entry: ChatEntry, fallbackCwd?: string) =>
    Effect.gen(function* () {
      const metaJson = entry.hasMeta
        ? Cursor.parseMetaJson(yield* readFileOrEmpty(`${entry.dir}/meta.json`))
        : undefined;
      if (!entry.hasStore) {
        return Cursor.summarizeStore({
          id: entry.id,
          workspaceHash: entry.workspaceHash,
          metaJson,
          fallbackCwd,
          mtimeMs: yield* mtimeMs(`${entry.dir}/meta.json`),
        });
      }
      const { meta, workspace } = yield* storeSummary(entry.dir).pipe(
        Effect.catchAll(() => Effect.succeed({ meta: undefined, workspace: undefined })),
      );
      return Cursor.summarizeStore({
        id: entry.id,
        workspaceHash: entry.workspaceHash,
        meta,
        metaJson,
        workspace,
        fallbackCwd,
        mtimeMs: yield* mtimeMs(`${entry.dir}/store.db`),
      });
    });

  const chatSession = (
    entry: ChatEntry,
    fallbackCwd?: string,
  ): Effect.Effect<Option.Option<Session>, StorageError> =>
    Effect.gen(function* () {
      const metaJson = entry.hasMeta
        ? Cursor.parseMetaJson(yield* readFileOrEmpty(`${entry.dir}/meta.json`))
        : undefined;
      if (!entry.hasStore) {
        return Option.some(
          Cursor.summarizeStore({
            id: entry.id,
            workspaceHash: entry.workspaceHash,
            metaJson,
            fallbackCwd,
            mtimeMs: yield* mtimeMs(`${entry.dir}/meta.json`),
          }),
        );
      }
      const db = yield* openStoreDb(`${entry.dir}/store.db`);
      try {
        const metaRow = db.all("select value from meta where key = '0'")[0];
        const meta = Cursor.parseStoreMeta(metaRow?.value);
        const blobs = new Map<string, Uint8Array>();
        for (const row of db.all("select id, data from blobs")) {
          if (typeof row.id === "string" && row.data instanceof Uint8Array) {
            blobs.set(row.id, row.data);
          }
        }
        const promptRaw = yield* readFileOrEmpty(`${entry.dir}/prompt_history.json`);
        return Option.some(
          Cursor.sessionFromStore({
            id: entry.id,
            workspaceHash: entry.workspaceHash,
            meta,
            metaJson,
            blobs,
            promptHistory: Cursor.parsePromptHistory(promptRaw),
            fallbackCwd,
          }),
        );
      } finally {
        db.close();
      }
    }).pipe(Effect.provide(fsLayer));

  const transcriptSession = (
    entry: TranscriptEntry,
    full: boolean,
  ): Effect.Effect<Session, unknown, Fs.FileSystem> =>
    Effect.gen(function* () {
      const raw = yield* readFileOrEmpty(entry.filePath);
      const source: Cursor.CursorTranscriptSource = {
        id: entry.id,
        projectSlug: entry.projectSlug,
        parentSessionId: entry.parentSessionId,
        mtimeMs: yield* mtimeMs(entry.filePath),
      };
      return full
        ? Cursor.fromTranscriptJsonl(raw, source)
        : Cursor.summarizeTranscriptJsonl(raw, source);
    });

  return {
    list: () =>
      Effect.gen(function* () {
        const entries = yield* scanEntries();
        const chatIds = new Set(
          entries.flatMap((entry) => (entry.kind === "chat" ? [entry.id] : [])),
        );
        // A transcript's project slug decodes to a real cwd — use it as the
        // fallback for chats whose store doesn't record one (the workspace
        // hash dir name is opaque).
        const transcriptCwd = new Map<string, string>();
        for (const entry of entries) {
          if (entry.kind === "transcript" && entry.parentSessionId === undefined) {
            transcriptCwd.set(entry.id, ClaudeCode.decodeProjectDir(entry.projectSlug));
          }
        }
        const sessions: Array<Session> = [];
        for (const entry of entries) {
          // A chat store.db wins over the lossy transcript of the same chat.
          if (entry.kind === "transcript" && chatIds.has(entry.id)) continue;
          const session = yield* (
            entry.kind === "chat"
              ? chatSummary(entry, transcriptCwd.get(entry.id))
              : transcriptSession(entry, false)
          ).pipe(Effect.option);
          if (Option.isSome(session)) sessions.push(session.value);
        }
        return sessions.sort((a, b) => b.lastActivityAt - a.lastActivityAt);
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to list cursor sessions")),
      ),

    getById: (id) =>
      Effect.gen(function* () {
        const entries = yield* scanEntries();
        const chatIds = new Set(
          entries.flatMap((entry) => (entry.kind === "chat" ? [entry.id] : [])),
        );
        for (const entry of entries) {
          if (entry.id !== id) continue;
          if (entry.kind === "chat") {
            const fallback = entries.find(
              (candidate) =>
                candidate.kind === "transcript" &&
                candidate.id === id &&
                candidate.parentSessionId === undefined,
            );
            const fallbackCwd =
              fallback === undefined || fallback.kind !== "transcript"
                ? undefined
                : ClaudeCode.decodeProjectDir(fallback.projectSlug);
            const session = yield* chatSession(entry, fallbackCwd);
            // The blob store may be pruned to nothing while the transcript
            // projection still holds the conversation — prefer whichever
            // decoding carries more nodes; keep the store's richer meta
            // (title, prompt history) when both are empty.
            if (fallback !== undefined && fallback.kind === "transcript") {
              const projected = yield* transcriptSession(fallback, true);
              if (Option.isNone(session) || projected.nodes.length > session.value.nodes.length) {
                return Option.some(projected);
              }
            }
            return session;
          }
          if (chatIds.has(id)) continue;
          return Option.some(yield* transcriptSession(entry, true));
        }
        return Option.none<Session>();
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to read cursor session")),
      ),

    hasSession: (id) =>
      scanEntries().pipe(
        Effect.map((entries) => entries.some((entry) => entry.id === id)),
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to check cursor session")),
      ),

    save: () => Effect.fail(new StorageError({ message: "Cursor repository is read-only" })),
    delete: () => Effect.fail(new StorageError({ message: "Cursor repository is read-only" })),
  };
};
