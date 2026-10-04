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
        return { sqlite, db: drizzle(sqlite, { schema }) };
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
      return { sqlite, db };
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

/**
 * Tables a real Devin store carries beyond the schema sepia writes. Both are
 * optional — a sepia-created store never gets them, and a live one may not
 * have migrated them in yet.
 */
interface DevinExtras {
  readonly hasToolCallState: boolean;
  readonly hasSubagentHeads: boolean;
}

const tableNames = (sqlite: Database): Set<string> =>
  new Set(
    sqlite
      .query<{ name: string }, []>("select name from sqlite_master where type = 'table'")
      .all()
      .map((row) => row.name),
  );

/**
 * The `tool_call_state` rows are serialised ACP `ToolCall`/`ToolCallUpdate`
 * objects; the update carries the authoritative `status` (the call row itself
 * is the still-open snapshot) plus `_meta["cognition.ai/terminal_exit"]` for
 * shell exit codes. Malformed rows are skipped — a single bad blob must not
 * take down the whole session read.
 */
const toolCallStateOutcomes = (
  sqlite: Database,
  sessionId: string,
): ReadonlyMap<string, Devin.ToolCallOutcome> => {
  const outcomes = new Map<string, Devin.ToolCallOutcome>();
  const rows = sqlite
    .query<{ tool_call_update_json: string | null }, [string]>(
      "select tool_call_update_json from tool_call_state where session_id = ?",
    )
    .all(sessionId);
  for (const row of rows) {
    const update = parseJson(row.tool_call_update_json) as Record<string, unknown> | null;
    const id = update?.toolCallId;
    const status = Devin.fromAcpToolCallStatus(update?.status);
    if (typeof id !== "string" || status === undefined) continue;
    const exit = (update?._meta as Record<string, unknown> | undefined)?.[
      "cognition.ai/terminal_exit"
    ] as Record<string, unknown> | undefined;
    const exitCode = typeof exit?.exit_code === "number" ? exit.exit_code : undefined;
    outcomes.set(id, {
      status,
      ...(exitCode !== undefined ? { exitCode } : {}),
    });
  }
  return outcomes;
};

const buildSession = (
  sessionRow: any,
  nodeRows: ReadonlyArray<any>,
  promptRows: ReadonlyArray<any>,
  extra?: {
    readonly outcomes?: ReadonlyMap<string, Devin.ToolCallOutcome>;
    readonly parentSessionId?: string | null;
  },
) =>
  Effect.try({
    try: () => {
      const parsed = nodeRows.map((row) =>
        Devin.parseChatMessage(
          parseJson(row.chatMessage),
          parseJson(row.metadata),
          row.nodeId,
          Option.fromNullable(row.parentNodeId),
          row.createdAt,
        ),
      );
      // Tool nodes carry each call's recorded outcome; `tool_call_state` rows
      // (when the table exists) are the authoritative lifecycle and override
      // them — they never carry durations, so result timing survives either way.
      const outcomes = new Map(Devin.toolNodeOutcomes(parsed));
      for (const [id, outcome] of extra?.outcomes ?? []) {
        outcomes.set(id, { ...outcomes.get(id), ...outcome });
      }
      const nodes = Devin.applyToolCallOutcomes(parsed, outcomes);

      const promptHistory = promptRows.map((row) =>
        PromptHistoryEntry.make({
          content: row.content,
          timestamp: row.timestamp,
          isShell: row.isShell === 1,
        }),
      );

      return Devin.sessionFromDevinRow(sessionRow, nodes, promptHistory, {
        parentSessionId: extra?.parentSessionId ?? null,
      });
    },
    catch: (error) => new StorageError({ message: `Failed to parse session: ${String(error)}` }),
  });

export const make = (
  dbPath: string,
  options: { readonly?: boolean } = {},
): Effect.Effect<SessionRepositoryService, StorageError> =>
  Effect.gen(function* () {
    const readonly = options.readonly === true;
    const { sqlite, db } = yield* openDb(dbPath, readonly);
    const tables = tableNames(sqlite);
    const extras: DevinExtras = {
      hasToolCallState: tables.has("tool_call_state"),
      hasSubagentHeads: tables.has("subagent_heads"),
    };

    /**
     * `subagent_heads` rows record `agent_id` = the spawned session's id and
     * `session_id` = the session that spawned it, so the parent of a session
     * is found by looking its own id up in `agent_id`.
     */
    const parentSessionId = (sessionId: string): string | null => {
      if (!extras.hasSubagentHeads) return null;
      const row = sqlite
        .query<{ session_id: string }, [string]>(
          "select session_id from subagent_heads where agent_id = ? limit 1",
        )
        .get(sessionId);
      return row?.session_id ?? null;
    };

    /** All known sub-agent links in one scan, for summary-only listings. */
    const subagentParents = (): ReadonlyMap<string, string> => {
      const parents = new Map<string, string>();
      if (!extras.hasSubagentHeads) return parents;
      for (const row of sqlite
        .query<{ session_id: string; agent_id: string }, []>(
          "select session_id, agent_id from subagent_heads",
        )
        .all()) {
        parents.set(row.agent_id, row.session_id);
      }
      return parents;
    };

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

                // Checkpoint refs have no native column; they ride in the
                // session metadata under a sepia-namespaced key so an
                // imported Cline session keeps its shadow-git pointers.
                const sessionMeta =
                  session.checkpoints.length === 0
                    ? session.metadata
                    : {
                        ...(typeof session.metadata === "object" && session.metadata !== null
                          ? (session.metadata as Record<string, unknown>)
                          : {}),
                        [Devin.SESSION_CHECKPOINTS_KEY]: session.checkpoints,
                      };

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
                    metadata: JSON.stringify(sessionMeta),
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

        const session = yield* buildSession(sessionRow, nodeRows, promptRows, {
          outcomes: extras.hasToolCallState ? toolCallStateOutcomes(sqlite, id) : undefined,
          parentSessionId: parentSessionId(id),
        });
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
        const parents = subagentParents();
        const sessions = yield* Effect.forEach(rows, (row) =>
          buildSession(row, [], [], { parentSessionId: parents.get(row.id) ?? null }).pipe(
            Effect.option,
          ),
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
