import { Effect, Layer, Option } from "effect";
import { statSync } from "node:fs";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Shared } from "sepia-core";
import * as Cursor from "./Cursor.js";
import { Session, StorageError } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";

/** Minimal sqlite surface a Cursor `store.db` needs — injectable for tests. */
export interface CursorStoreDb {
  readonly all: (
    sql: string,
    params?: ReadonlyArray<unknown>,
  ) => ReadonlyArray<Record<string, unknown>>;
  /** Present only when the store was opened writable — INSERT/CREATE. */
  readonly run?: (sql: string, params?: ReadonlyArray<unknown>) => void;
  readonly close: () => void;
}

export type OpenStoreDb = (path: string) => Effect.Effect<CursorStoreDb, StorageError>;

export interface CursorRepositoryOptions {
  /** Cursor data dir, usually `~/.cursor`. */
  readonly cursorDir: string;
  /** Opens a `store.db` read-only; defaults to `bun:sqlite` loaded lazily. */
  readonly openStoreDb?: OpenStoreDb;
  /**
   * Opens a `store.db` for writing; defaults to `bun:sqlite` read-write.
   * When only `openStoreDb` is injected it doubles as the writable opener
   * (a test fake that implements `run` is writable); `save` then fails
   * cleanly when the injected store has no `run`.
   */
  readonly openWritableStoreDb?: OpenStoreDb;
}

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const storageError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/**
 * Ids and slugs become path segments (`<chat-id>/<chat-id>.jsonl`), so they
 * must be a single safe file name — no separators, NUL, or dot-dirs.
 */
const isSafeFileName = (name: string): boolean =>
  name !== "" && name !== "." && name !== ".." && !/[/\\\0]/.test(name);

/** The slice of `bun:sqlite`'s Database a store.db read needs. */
interface BunSqliteModule {
  readonly Database: new (
    path: string,
    options?: { readonly readonly?: boolean },
  ) => {
    readonly query: (sql: string) => {
      readonly all: (...params: ReadonlyArray<unknown>) => unknown[];
    };
    readonly run: (sql: string, params?: ReadonlyArray<unknown>) => unknown;
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

/**
 * Read-write open for `save` — `new Database(path)` creates the file when
 * the chat dir exists but the store was never written (a 0-byte file is a
 * fresh database either way).
 */
const defaultOpenWritableStoreDb: OpenStoreDb = (dbPath) =>
  Effect.tryPromise({
    try: async () => {
      const { Database } = (await import(BUN_SQLITE_MODULE)) as BunSqliteModule;
      const sqlite = new Database(dbPath);
      return {
        all: (sql, params = []) =>
          sqlite.query(sql).all(...params) as ReadonlyArray<Record<string, unknown>>,
        run: (sql, params = []) => {
          sqlite.run(sql, [...params]);
        },
        close: () => sqlite.close(),
      };
    },
    catch: (cause) =>
      new StorageError({
        message: `Failed to open cursor store ${dbPath} for writing: ${cause instanceof Error ? cause.message : String(cause)}`,
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
 * SessionRepository over Cursor's two on-disk stores:
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
 *
 * `save` writes a top-level session canonically: the chat dir resolves to
 * `chats/<md5(cwd)>/<id>` (an existing chats dir for the id, under any
 * workspace hash, is updated in place), `store.db` gets the session's
 * message blobs plus a fresh checkpoint and meta root — blob inserts are
 * additive, so rewriting keeps the store's older DAG entries — and the
 * `meta.json`/`prompt_history.json` sidecars follow. The transcript
 * projection is written alongside, matching Cursor's own dual write.
 * Subagent sessions have no chats-store concept and stay transcript-only.
 * `delete` removes both the chat store dir and the transcript dir for the
 * id.
 */
export const makeCursorSessionRepository = (
  options: CursorRepositoryOptions,
): SessionRepositoryService => {
  const openStoreDb = options.openStoreDb ?? defaultOpenStoreDb;
  // An injected read-only opener doubles as the writable one in tests; with
  // nothing injected the real writable `bun:sqlite` path is used.
  const openWritableStoreDb =
    options.openWritableStoreDb ?? options.openStoreDb ?? defaultOpenWritableStoreDb;
  const chatsDir = () => `${options.cursorDir}/chats`;
  const projectsDir = () => `${options.cursorDir}/projects`;

  /** `fs.exists` that degrades errors to false — a stat failure reads as absent. */
  const existsOrFalse = (filePath: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      return yield* fs.exists(filePath).pipe(Effect.orElseSucceed(() => false));
    });

  const listDir = (dir: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      if (!(yield* existsOrFalse(dir))) return [] as ReadonlyArray<string>;
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
          if (yield* existsOrFalse(main)) {
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

  /** statSync mtime — a missing/unstatable file reads as `undefined`. */
  const fileMtimeMs = (filePath: string): number | undefined => {
    try {
      return statSync(filePath).mtimeMs;
    } catch {
      return undefined;
    }
  };

  /**
   * Read cache — `list()` opens every chat's `store.db` and reads every
   * transcript on each call. The entry scan is the cheap half, so the
   * summarized array is keyed on a stamp built from it: each chat dir's
   * `store.db` (+ its WAL — external writes may touch only that, like the
   * devin store) and sidecar mtimes, each transcript's mtime, and the
   * entry set itself. Writes through this instance invalidate explicitly.
   */
  let listCache: { readonly stamp: string; readonly value: ReadonlyArray<Session> } | undefined;
  const listStamp = (entries: ReadonlyArray<StoreEntry>): string => {
    let maxMtime = 0;
    const parts: Array<string> = [];
    const bump = (filePath: string): string => {
      const mtime = fileMtimeMs(filePath);
      if (mtime !== undefined && mtime > maxMtime) maxMtime = mtime;
      return `${filePath}:${mtime ?? "?"}`;
    };
    for (const entry of entries) {
      if (entry.kind === "chat") {
        parts.push(
          `${entry.dir}:${entry.hasStore}:${entry.hasMeta}`,
          bump(`${entry.dir}/store.db`),
          bump(`${entry.dir}/store.db-wal`),
          bump(`${entry.dir}/meta.json`),
          bump(`${entry.dir}/prompt_history.json`),
        );
      } else {
        parts.push(bump(entry.filePath));
      }
    }
    return `${entries.length}:${maxMtime}\n${parts.join("\n")}`;
  };

  const mtimeMs = (filePath: string) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const info = yield* fs.stat(filePath).pipe(Effect.option);
      return Option.flatMap(info, (i) => i.mtime).pipe(
        Option.map((d) => d.getTime()),
        Option.getOrUndefined,
      );
    });

  /**
   * `db.all` that degrades a throw to `[]` — a 0-byte or schema-less
   * `store.db` (real `chats/` dirs hold them) opens fine but has no
   * tables, and a sync throw here is a defect `catchAll` can't rescue.
   */
  const allSafe = (
    db: CursorStoreDb,
    sql: string,
    params: ReadonlyArray<unknown> = [],
  ): ReadonlyArray<Record<string, unknown>> => {
    try {
      return db.all(sql, params);
    } catch {
      return [];
    }
  };

  /**
   * The prior `meta['0']` row for a `save` rewrite — `undefined` whenever
   * the store is missing, empty or unopenable (a fresh chat dir has no
   * meta row yet).
   */
  const storeMetaRow = (storePath: string) =>
    openStoreDb(storePath).pipe(
      Effect.map((db) => {
        try {
          return Cursor.parseStoreMeta(
            allSafe(db, "select value from meta where key = '0'")[0]?.value,
          );
        } finally {
          db.close();
        }
      }),
      Effect.catchAll(() => Effect.succeed(undefined)),
    );

  /** meta['0'] plus the root checkpoint's workspace, from an open store. */
  const storeSummary = (chatDir: string) =>
    Effect.gen(function* () {
      const db = yield* openStoreDb(`${chatDir}/store.db`);
      try {
        const metaRow = allSafe(db, "select value from meta where key = '0'")[0];
        const meta = Cursor.parseStoreMeta(metaRow?.value);
        let workspace: string | undefined;
        const rootId = meta?.latestRootBlobId;
        if (rootId !== undefined && rootId !== "") {
          const row = allSafe(db, "select data from blobs where id = ?", [rootId])[0];
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
        const metaRow = allSafe(db, "select value from meta where key = '0'")[0];
        const meta = Cursor.parseStoreMeta(metaRow?.value);
        const blobs = new Map<string, Uint8Array>();
        for (const row of allSafe(db, "select id, data from blobs")) {
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
        const stamp = listStamp(entries);
        if (listCache?.stamp === stamp) return listCache.value;
        const chatIds = new Set(
          entries.flatMap((entry) => (entry.kind === "chat" ? [entry.id] : [])),
        );
        // A transcript's project slug decodes to a real cwd — use it as the
        // fallback for chats whose store doesn't record one (the workspace
        // hash dir name is opaque).
        const transcriptCwd = new Map<string, string>();
        for (const entry of entries) {
          if (entry.kind === "transcript" && entry.parentSessionId === undefined) {
            transcriptCwd.set(entry.id, Shared.decodeProjectDir(entry.projectSlug));
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
        const listed = sessions.sort((a, b) => b.lastActivityAt - a.lastActivityAt);
        listCache = { stamp, value: listed };
        return listed;
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
                : Shared.decodeProjectDir(fallback.projectSlug);
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

    save: (session) =>
      Effect.gen(function* () {
        const path = yield* Path.Path;
        const fs = yield* Fs.FileSystem;
        const parentId = Option.getOrUndefined(session.parentSessionId);
        for (const unsafe of [session.id, parentId]) {
          if (unsafe !== undefined && !isSafeFileName(unsafe)) {
            return yield* Effect.fail(
              new StorageError({
                message: `Cursor session id is not a safe file name: ${JSON.stringify(unsafe)}`,
              }),
            );
          }
        }

        if (parentId === undefined) {
          // Canonical write — the chats store.db. An existing chats dir
          // for the id (under any workspace hash) is updated in place;
          // otherwise the dir is `chats/<md5(cwd)>/<id>`, the same mapping
          // Cursor itself uses.
          let chatDir: string | undefined;
          for (const wsHash of yield* listDir(chatsDir())) {
            const candidate = path.join(chatsDir(), wsHash, session.id);
            const info = yield* fs.stat(candidate).pipe(Effect.option);
            if (Option.isSome(info) && info.value.type === "Directory") {
              chatDir = candidate;
              break;
            }
          }
          chatDir ??= path.join(
            chatsDir(),
            Cursor.workspaceHashFromCwd(session.workingDirectory),
            session.id,
          );
          yield* fs.makeDirectory(chatDir, { recursive: true });

          const storePath = path.join(chatDir, "store.db");
          // A rewrite keeps the chat's original creation stamp — the
          // meta.json sidecar records it, the meta row when the sidecar
          // was never written.
          const priorMeta = Cursor.parseMetaJson(
            yield* readFileOrEmpty(path.join(chatDir, "meta.json")),
          );
          const priorStore = yield* storeMetaRow(storePath);
          const plan = Cursor.storeWritePlan(session, {
            createdAtMs: priorMeta?.createdAtMs ?? priorStore?.createdAt,
          });

          const db = yield* openWritableStoreDb(storePath);
          if (db.run === undefined) {
            db.close();
            return yield* Effect.fail(
              new StorageError({
                message: `Cursor store ${storePath} was opened without a write surface`,
              }),
            );
          }
          const run = db.run;
          yield* Effect.try({
            try: () => {
              try {
                // the same pragmas + schema the agent's own
                // `initializeDriver` runs (user_version 1, WAL)
                run("PRAGMA journal_mode = WAL");
                run("PRAGMA synchronous = NORMAL");
                run("PRAGMA busy_timeout = 5000");
                run("PRAGMA user_version = 1");
                run("CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB)");
                run("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)");
                for (const blob of plan.blobs) {
                  run("INSERT OR REPLACE INTO blobs (id, data) VALUES (?, ?)", [
                    blob.id,
                    blob.data,
                  ]);
                }
                run("INSERT OR REPLACE INTO meta (key, value) VALUES ('0', ?)", [plan.metaRow]);
              } finally {
                db.close();
              }
            },
            catch: (cause) =>
              new StorageError({
                message: `Failed to write cursor store ${storePath}: ${cause instanceof Error ? cause.message : String(cause)}`,
              }),
          });

          yield* fs.writeFileString(path.join(chatDir, "meta.json"), plan.metaJson);
          if (plan.promptHistoryJson !== undefined) {
            yield* fs.writeFileString(
              path.join(chatDir, "prompt_history.json"),
              plan.promptHistoryJson,
            );
          }
        }

        // Reuse the project slug a transcript read recorded; otherwise
        // derive it from the working directory.
        const meta = session.metadata;
        const recorded =
          isObject(meta) && meta.store === "transcript" && typeof meta.project === "string"
            ? meta.project
            : undefined;
        const slug =
          recorded !== undefined && isSafeFileName(recorded)
            ? recorded
            : Cursor.projectSlugFromCwd(session.workingDirectory);

        let filePath: string;
        if (parentId === undefined) {
          const dir = path.join(projectsDir(), slug, "agent-transcripts", session.id);
          yield* fs.makeDirectory(dir, { recursive: true });
          filePath = path.join(dir, `${session.id}${JSONL}`);
        } else {
          // A subagent transcript lives under its parent chat's dir — find
          // the project that already holds the parent, else fall back to
          // this session's own project.
          let parentSlug = slug;
          for (const candidate of yield* listDir(projectsDir())) {
            const parentDir = path.join(projectsDir(), candidate, "agent-transcripts", parentId);
            if (yield* existsOrFalse(parentDir)) {
              parentSlug = candidate;
              break;
            }
          }
          const dir = path.join(
            projectsDir(),
            parentSlug,
            "agent-transcripts",
            parentId,
            "subagents",
          );
          yield* fs.makeDirectory(dir, { recursive: true });
          filePath = path.join(dir, `${session.id}${JSONL}`);
        }

        yield* fs.writeFileString(filePath, Cursor.toTranscriptJsonl(session));
        // The projection has no timestamps; the reader falls back to the
        // file mtime, so stamp it with the session's last activity. A
        // failed stamp degrades timestamps to "now", never the write.
        yield* fs
          .utimes(filePath, new Date(), new Date(session.lastActivityAt * 1000))
          .pipe(Effect.ignore);
        listCache = undefined;
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to save cursor session")),
      ),

    delete: (id) =>
      Effect.gen(function* () {
        const path = yield* Path.Path;
        const fs = yield* Fs.FileSystem;
        if (!isSafeFileName(id)) {
          return yield* Effect.fail(
            new StorageError({
              message: `Cursor session id is not a safe file name: ${JSON.stringify(id)}`,
            }),
          );
        }
        const removeIfExists = (target: string, recursive: boolean) =>
          Effect.gen(function* () {
            if (yield* existsOrFalse(target)) {
              yield* fs.remove(target, { recursive });
            }
          });

        for (const wsHash of yield* listDir(chatsDir())) {
          yield* removeIfExists(path.join(chatsDir(), wsHash, id), true);
        }
        for (const slug of yield* listDir(projectsDir())) {
          const transcriptsDir = path.join(projectsDir(), slug, "agent-transcripts");
          // The chat's own transcript dir (main file + its subagents)…
          yield* removeIfExists(path.join(transcriptsDir, id), true);
          // …and a subagent file of the same id under another chat.
          for (const chatId of yield* listDir(transcriptsDir)) {
            yield* removeIfExists(
              path.join(transcriptsDir, chatId, "subagents", `${id}${JSONL}`),
              false,
            );
          }
        }
        listCache = undefined;
      }).pipe(
        Effect.provide(fsLayer),
        Effect.mapError(storageError("Failed to delete cursor session")),
      ),
  };
};
