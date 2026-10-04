import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Context, Effect, Layer, Option } from "effect";
import * as Cline from "./Cline.js";
import * as ClineIndex from "./ClineIndex.js";
import { ConversionError, Session } from "./Domain.js";
import type { SessionSqlite } from "./SessionSqlite.js";

const register = (
  openDb: OpenSessionSqlite,
  dbPath: string,
  row: ClineIndex.SessionRow,
): Effect.Effect<void, ConversionError> =>
  Effect.try({
    try: () => {
      const sqlite = openDb(dbPath, false);
      try {
        sqlite.run(ClineIndex.SESSIONS_DDL);
        sqlite.insertSession(
          [...ClineIndex.SESSION_COLUMNS],
          ClineIndex.SESSION_COLUMNS.map((column) => row[column]),
        );
      } finally {
        sqlite.close();
      }
    },
    catch: (error) =>
      new ConversionError({
        message: `Failed to register the session in ${dbPath}: ${String(error)}`,
        cause: error,
      }),
  });

/** True while a live process still owns the index row: never silently replace it. */
const ownerIsAlive = (row: { status: string; pid: number }): boolean =>
  ClineIndex.isActiveRow(row, ClineIndex.isPidAlive(row.pid));

/** The session's index row (`{status, pid}`) — `Option.none` when the index is absent or has no row. */
export const indexRow = (
  openDb: OpenSessionSqlite,
  fs: Fs.FileSystem,
  dbPath: string,
  sessionId: string,
): Effect.Effect<Option.Option<{ status: string; pid: number }>, ConversionError> =>
  Effect.gen(function* () {
    const exists = yield* fs.exists(dbPath).pipe(
      Effect.mapError(
        (error) =>
          new ConversionError({
            message: `Failed to read the Cline session index at ${dbPath}: ${String(error)}`,
            cause: error,
          }),
      ),
    );
    if (!exists) {
      return Option.none();
    }
    return yield* Effect.try({
      try: () => {
        const sqlite = openDb(dbPath, true);
        try {
          if (!sqlite.allTables().includes("sessions")) {
            return Option.none();
          }
          const row = sqlite.get<{ status: string; pid: number }>(
            "select status, pid from sessions where session_id = ?",
            sessionId,
          );
          return row === null ? Option.none() : Option.some(row);
        } finally {
          sqlite.close();
        }
      },
      catch: (error) =>
        new ConversionError({
          message: `Failed to read the Cline session index at ${dbPath}: ${String(error)}`,
          cause: error,
        }),
    });
  });

export interface ClineStoreService {
  /**
   * Write the session artifacts into `<dataDir>/sessions/<session-id>/` and add
   * its row to `<dataDir>/db/sessions.db` so `cline --id <session-id>` resumes it.
   * Refuses to overwrite a session that still belongs to a live owner unless
   * `force` is set.
   */
  readonly install: (
    session: Session,
    sessionId?: string,
    options?: { readonly force?: boolean },
  ) => Effect.Effect<string, ConversionError, Fs.FileSystem | Path.Path>;
}

/** Opens the session index; `readonly` must never write or create. */
export type OpenSessionSqlite = (dbPath: string, readonly: boolean) => SessionSqlite;

export class ClineStore extends Context.Tag("ClineStore")<ClineStore, ClineStoreService>() {}

export const make = (openDb: OpenSessionSqlite, dataDir: string): ClineStoreService => ({
  install: (session, sessionId, options) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const path = yield* Path.Path;

      const id = sessionId ?? Cline.clineSessionId(session.createdAt * 1000);
      const dir = path.join(dataDir, "sessions", id);
      const messagesPath = path.join(dir, `${id}.messages.json`);
      const dbDir = path.join(dataDir, "db");
      const dbPath = path.join(dbDir, "sessions.db");

      const existing = yield* indexRow(openDb, fs, dbPath, id);
      if (Option.isSome(existing) && ownerIsAlive(existing.value)) {
        if (options?.force !== true) {
          return yield* Effect.fail(
            new ConversionError({
              message:
                `Session ${id} still belongs to a live owner ` +
                `(status ${existing.value.status}, pid ${existing.value.pid}); resume it or retry with --force`,
              cause: null,
            }),
          );
        }
      }

      yield* fs.makeDirectory(dir, { recursive: true });
      yield* fs.writeFileString(
        path.join(dir, `${id}.json`),
        JSON.stringify(Cline.sessionManifest(session, id, messagesPath), null, 2),
      );
      yield* fs.writeFileString(
        messagesPath,
        JSON.stringify(Cline.sessionMessages(session, id), null, 2),
      );

      yield* fs.makeDirectory(dbDir, { recursive: true });
      yield* register(openDb, dbPath, ClineIndex.sessionRow(session, id, messagesPath));

      return id;
    }).pipe(
      Effect.mapError((error) =>
        error instanceof ConversionError
          ? error
          : new ConversionError({
              message: `Cline install failed: ${String(error)}`,
              cause: error,
            }),
      ),
    ),
});

export const layer = (openDb: OpenSessionSqlite, dataDir: string): Layer.Layer<ClineStore> =>
  Layer.succeed(ClineStore, make(openDb, dataDir));
