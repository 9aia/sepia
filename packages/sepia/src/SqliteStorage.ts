import { desc, eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/bun-sqlite";
import { migrate } from "drizzle-orm/bun-sqlite/migrator";
import { Database } from "bun:sqlite";
import { Effect, Layer, Option } from "effect";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PromptHistoryEntry, Session, StorageError } from "./Domain.js";
import * as Devin from "./Devin.js";
import * as schema from "./DbSchema.js";
import { SessionRepository, needsMigration, type SessionRepositoryService } from "./Storage.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const migrationsFolder = path.resolve(__dirname, "..", "drizzle");

const openDb = (dbPath: string, readonly: boolean) =>
  Effect.try({
    try: () => {
      const sqlite = new Database(dbPath, readonly ? { readonly: true } : undefined);
      sqlite.run("PRAGMA busy_timeout = 5000;");

      // Read paths (list/export/install) must leave a live store untouched, so
      // they skip the WAL switch and the migration bookkeeping entirely.
      if (readonly) {
        return drizzle(sqlite, { schema });
      }

      if (dbPath !== ":memory:") {
        sqlite.run("PRAGMA journal_mode = WAL;");
      }

      const db = drizzle(sqlite, { schema });
      const tables = new Set(
        sqlite
          .query<{ name: string }, []>("select name from sqlite_master where type = 'table'")
          .all()
          .map((row) => row.name),
      );
      if (needsMigration(tables)) {
        migrate(db, { migrationsFolder });
      }
      return db;
    },
    catch: (error) => new StorageError({ message: `Failed to open database: ${String(error)}` }),
  });

const parseJson = (s: string | null | undefined) => {
  if (!s) return null;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
};

const buildSession = (
  sessionRow: any,
  nodeRows: ReadonlyArray<any>,
  promptRows: ReadonlyArray<any>,
) =>
  Effect.try({
    try: () => {
      const nodes = nodeRows.map((row) =>
        Devin.parseChatMessage(
          parseJson(row.chatMessage),
          parseJson(row.metadata),
          row.nodeId,
          Option.fromNullable(row.parentNodeId),
          row.createdAt,
        ),
      );

      const promptHistory = promptRows.map((row) =>
        PromptHistoryEntry.make({
          content: row.content,
          timestamp: row.timestamp,
          isShell: row.isShell === 1,
        }),
      );

      return Devin.sessionFromDevinRow(sessionRow, nodes, promptHistory);
    },
    catch: (error) => new StorageError({ message: `Failed to parse session: ${String(error)}` }),
  });

export const make = (
  dbPath: string,
  options: { readonly?: boolean } = {},
): Effect.Effect<SessionRepositoryService, StorageError> =>
  Effect.gen(function* () {
    const readonly = options.readonly === true;
    const db = yield* openDb(dbPath, readonly);

    const save = (session: Session) =>
      readonly
        ? Effect.fail(
            new StorageError({
              message: `Store is open read-only, cannot save session ${session.id}`,
            }),
          )
        : Effect.try({
            try: () => {
              db.transaction((tx) => {
                tx.delete(schema.messageNodes)
                  .where(eq(schema.messageNodes.sessionId, session.id))
                  .run();
                tx.delete(schema.promptHistory)
                  .where(eq(schema.promptHistory.sessionId, session.id))
                  .run();
                tx.delete(schema.sessions).where(eq(schema.sessions.id, session.id)).run();

                tx.insert(schema.sessions)
                  .values({
                    id: session.id,
                    workingDirectory: session.workingDirectory,
                    backendType: session.backendType,
                    model: session.model,
                    agentMode: session.agentMode,
                    createdAt: session.createdAt,
                    lastActivityAt: session.lastActivityAt,
                    title: session.title,
                    mainChainId: session.mainChainId,
                    shellLastSeenIndex: session.shellLastSeenIndex,
                    cogsJson: session.cogsJson,
                    workspaceDirs: session.workspaceDirs,
                    hidden: session.hidden,
                    metadata: JSON.stringify(session.metadata),
                  })
                  .run();

                for (const node of session.nodes) {
                  tx.insert(schema.messageNodes)
                    .values({
                      sessionId: session.id,
                      nodeId: node.nodeId,
                      parentNodeId: Option.getOrUndefined(node.parentNodeId),
                      chatMessage: JSON.stringify(Devin.buildChatMessage(node, session.model)),
                      createdAt: node.createdAt,
                      metadata:
                        node.metadata === null || node.metadata === undefined
                          ? null
                          : JSON.stringify(node.metadata),
                    })
                    .run();
                }

                for (const ph of session.promptHistory) {
                  tx.insert(schema.promptHistory)
                    .values({
                      sessionId: session.id,
                      content: ph.content,
                      timestamp: ph.timestamp,
                      isShell: ph.isShell ? 1 : 0,
                    })
                    .run();
                }
              });
            },
            catch: (error) =>
              new StorageError({ message: `Failed to save session: ${String(error)}` }),
          });

    const getById = (id: string) =>
      Effect.gen(function* () {
        const sessionRow = db
          .select()
          .from(schema.sessions)
          .where(eq(schema.sessions.id, id))
          .get();
        if (!sessionRow) return Option.none<Session>();

        const nodeRows = db
          .select()
          .from(schema.messageNodes)
          .where(eq(schema.messageNodes.sessionId, id))
          .orderBy(schema.messageNodes.nodeId)
          .all();

        const promptRows = db
          .select()
          .from(schema.promptHistory)
          .where(eq(schema.promptHistory.sessionId, id))
          .orderBy(schema.promptHistory.id)
          .all();

        const session = yield* buildSession(sessionRow, nodeRows, promptRows);
        return Option.some(session);
      }).pipe(
        Effect.mapError(
          (error) => new StorageError({ message: `Failed to read session: ${String(error)}` }),
        ),
      );

    // Summaries only: a real store holds gigabytes of message nodes, so listing
    // must not read them. Unreadable rows are skipped rather than failing the list.
    const list = () =>
      Effect.gen(function* () {
        const rows = db
          .select()
          .from(schema.sessions)
          .orderBy(desc(schema.sessions.lastActivityAt))
          .all();
        const sessions = yield* Effect.forEach(rows, (row) =>
          buildSession(row, [], []).pipe(Effect.option),
        );
        return sessions.flatMap((opt) => (Option.isSome(opt) ? [opt.value] : []));
      }).pipe(
        Effect.mapError(
          (error) => new StorageError({ message: `Failed to list sessions: ${String(error)}` }),
        ),
      );

    const delete_ = (id: string) =>
      readonly
        ? Effect.fail(
            new StorageError({ message: `Store is open read-only, cannot delete session ${id}` }),
          )
        : Effect.try({
            try: () => {
              db.transaction((tx) => {
                tx.delete(schema.messageNodes).where(eq(schema.messageNodes.sessionId, id)).run();
                tx.delete(schema.promptHistory).where(eq(schema.promptHistory.sessionId, id)).run();
                tx.delete(schema.sessions).where(eq(schema.sessions.id, id)).run();
              });
            },
            catch: (error) =>
              new StorageError({ message: `Failed to delete session: ${String(error)}` }),
          });

    const hasSession = (id: string) =>
      Effect.map(getById(id), Option.isSome).pipe(
        Effect.mapError(
          (error) => new StorageError({ message: `Failed to check session: ${String(error)}` }),
        ),
      );

    return SessionRepository.of({ save, getById, list, delete: delete_, hasSession });
  });

export const layer = (dbPath: string): Layer.Layer<SessionRepository, StorageError> =>
  Layer.effect(SessionRepository, make(dbPath));

/** Read-only layer: opens (and never migrates or writes) an existing store. */
export const layerReadonly = (dbPath: string): Layer.Layer<SessionRepository, StorageError> =>
  Layer.effect(SessionRepository, make(dbPath, { readonly: true }));
