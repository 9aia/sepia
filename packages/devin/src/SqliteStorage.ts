import { desc, eq } from "drizzle-orm";
import { drizzle } from "drizzle-orm/bun-sqlite";
import { migrate } from "drizzle-orm/bun-sqlite/migrator";
import { Database } from "bun:sqlite";
import { Effect, Layer, Option } from "effect";
import path from "node:path";
import { statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  type MessageNode,
  PromptHistoryEntry,
  Session,
  type SessionNodeWindow,
  StorageError,
} from "sepia-core";
import * as Devin from "./Devin.js";
import * as schema from "./DbSchema.js";
import { SessionRepository, needsMigration, type SessionRepositoryService } from "sepia-core";

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
     * Read caches — the store is re-read on every list/history/export/stream
     * call and a live session's node table can hold tens of thousands of rows,
     * so parsing must not repeat when nothing changed.
     *
     * `list` is keyed on the newest mtime of the db file and its WAL (WAL
     * mode means external writes touch `*-wal` while the main file's mtime
     * stays stale). `:memory:` stores can't be written by another process,
     * so a constant stamp is safe there — writes through this instance
     * invalidate explicitly.
     *
     * `getById` is keyed on (lastActivityAt, nodeCount, promptCount) —
     * three indexed queries — so history paging, export and stream hits
     * parse the node graph only once per change.
     */
    const dbStamp = (): number => {
      if (dbPath === ":memory:") return 0;
      let stamp = 0;
      for (const suffix of ["", "-wal"]) {
        try {
          stamp = Math.max(stamp, statSync(dbPath + suffix).mtimeMs);
        } catch {
          // missing wal/main — stamp stays low
        }
      }
      return stamp;
    };
    let listCache: { readonly stamp: number; readonly value: ReadonlyArray<Session> } | undefined;
    const SESSION_CACHE_MAX = 64;
    const sessionCache = new Map<string, { readonly key: string; readonly session: Session }>();
    /**
     * Call-bearing nodes per session — the calls join for history windows.
     * `message_nodes` is append-only and rows are immutable blobs, so the
     * index is exact when keyed on `max(node_id)`: any append bumps it.
     */
    const callsIndex = new Map<
      string,
      { readonly maxNodeId: number; readonly nodes: ReadonlyArray<MessageNode> }
    >();
    const invalidateCaches = () => {
      listCache = undefined;
      sessionCache.clear();
      callsIndex.clear();
    };

    interface SessionStamp {
      readonly lastActivityAt: number;
      readonly backendType: string;
      readonly nodeCount: number;
      readonly promptCount: number;
      readonly toolStateCount: number;
      readonly maxNodeId: number;
    }
    /**
     * One indexed probe serving both caches — the session row's activity
     * stamp plus per-table counts and the newest node id. `None` = the id
     * isn't in this store.
     */
    const sessionStamp = (id: string): SessionStamp | null =>
      sqlite
        .query<SessionStamp, [string]>(
          `select s.last_activity_at as lastActivityAt, s.backend_type as backendType,
                  (select count(*) from message_nodes where session_id = s.id) as nodeCount,
                  (select count(*) from prompt_history where session_id = s.id) as promptCount,
                  (select coalesce(max(node_id), 0) from message_nodes where session_id = s.id) as maxNodeId,
                  ${
                    extras.hasToolCallState
                      ? "(select count(*) from tool_call_state where session_id = s.id)"
                      : "0"
                  } as toolStateCount
           from sessions s where s.id = ?`,
        )
        .get(id) ?? null;

    const sessionCacheKey = (stamp: SessionStamp): string =>
      `${stamp.lastActivityAt}:${stamp.nodeCount}:${stamp.promptCount}:${stamp.toolStateCount}`;

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
              invalidateCaches();
            },
            catch: (error) =>
              new StorageError({ message: `Failed to save session: ${String(error)}` }),
          });

    const getById = (id: string) =>
      Effect.gen(function* () {
        // Cheap cache key — the full row + every node only load on a miss.
        // tool_call_state appends ride the key so live outcome updates
        // can't go stale.
        const stamp = sessionStamp(id);
        if (!stamp) return Option.none<Session>();
        const cacheKey = sessionCacheKey(stamp);
        const hit = sessionCache.get(id);
        if (hit?.key === cacheKey) return Option.some(hit.session);

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
        if (sessionCache.size >= SESSION_CACHE_MAX) sessionCache.clear();
        sessionCache.set(id, { key: cacheKey, session });
        return Option.some(session);
      }).pipe(
        Effect.mapError(
          (error) => new StorageError({ message: `Failed to read session: ${String(error)}` }),
        ),
      );

    // Summaries only: a real store holds gigabytes of message nodes, so listing
    // must not read them. `cogs_json` is also projected out — resume blobs are
    // megabytes per row and no list consumer reads them (buildSession defaults
    // the absent column to "[]"). Unreadable rows are skipped, not fatal.
    const list = () =>
      Effect.gen(function* () {
        const stamp = dbStamp();
        if (listCache?.stamp === stamp) return listCache.value;
        const rows = db
          .select({
            id: schema.sessions.id,
            workingDirectory: schema.sessions.workingDirectory,
            backendType: schema.sessions.backendType,
            model: schema.sessions.model,
            agentMode: schema.sessions.agentMode,
            createdAt: schema.sessions.createdAt,
            lastActivityAt: schema.sessions.lastActivityAt,
            title: schema.sessions.title,
            mainChainId: schema.sessions.mainChainId,
            shellLastSeenIndex: schema.sessions.shellLastSeenIndex,
            workspaceDirs: schema.sessions.workspaceDirs,
            hidden: schema.sessions.hidden,
            metadata: schema.sessions.metadata,
          })
          .from(schema.sessions)
          .orderBy(desc(schema.sessions.lastActivityAt))
          .all();
        const parents = subagentParents();
        const sessions = yield* Effect.forEach(rows, (row) =>
          buildSession(row, [], [], { parentSessionId: parents.get(row.id) ?? null }).pipe(
            Effect.option,
          ),
        );
        const listed = sessions.flatMap((opt) => (Option.isSome(opt) ? [opt.value] : []));
        listCache = { stamp, value: listed };
        return listed;
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
              invalidateCaches();
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

    /**
     * SQL-paged history read — the window's `start/limit/before` arithmetic
     * mirrors `ControlPlane.getHistory`. Three tiers, cheapest first:
     *  - a warm `sessionCache` entry slices in memory (no SQL);
     *  - otherwise the window is a LIMIT/OFFSET query — no backlog parse;
     *  - the calls the window's tool rows reference come from `callsIndex`,
     *    keyed on `max(node_id)` (append-only store ⇒ an append bumps it,
     *    so the index is exact) and built by one call-bearing scan only
     *    when the window actually holds tool rows.
     */
    const nodesWindow = (
      id: string,
      options: { readonly limit?: number; readonly before?: number },
    ) =>
      Effect.try({
        try: () => {
          const stamp = sessionStamp(id);
          if (!stamp) return Option.none<SessionNodeWindow>();

          const total = stamp.nodeCount;
          const before =
            options.before !== undefined && Number.isFinite(options.before)
              ? Math.min(Math.max(0, Math.floor(options.before)), total)
              : total;
          const limit = Math.max(1, Math.floor(options.limit ?? total));
          const start = Math.max(0, before - limit);

          // Warm whole-session cache → slice in memory.
          const hit = sessionCache.get(id);
          if (hit?.key === sessionCacheKey(stamp)) {
            const all = hit.session.nodes;
            return Option.some<SessionNodeWindow>({
              nodes: all.slice(start, before),
              toolCallNodes: all.filter((n) => n.toolCalls.length > 0),
              total,
              start,
              backendType: stamp.backendType,
            });
          }

          const parseRows = (
            rows: ReadonlyArray<{
              node_id: number;
              parent_node_id: number | null;
              chat_message: string;
              created_at: number;
              metadata: string | null;
            }>,
          ) =>
            rows.map((row) =>
              Devin.parseChatMessage(
                parseJson(row.chat_message),
                parseJson(row.metadata),
                row.node_id,
                Option.fromNullable(row.parent_node_id),
                row.created_at,
              ),
            );

          const windowRows = sqlite
            .query<
              {
                node_id: number;
                parent_node_id: number | null;
                chat_message: string;
                created_at: number;
                metadata: string | null;
              },
              [string, number, number]
            >(
              `select node_id, parent_node_id, chat_message, created_at, metadata
             from message_nodes where session_id = ? order by node_id limit ? offset ?`,
            )
            .all(id, before - start, start);

          const nodes = parseRows(windowRows);
          const toolCallNodes = nodes.filter((n) => n.toolCalls.length > 0);

          // Resolve calls only when the window actually has tool rows.
          const needsCalls = nodes.some((n) => n.role === "tool");
          if (needsCalls) {
            const inWindow = new Set(nodes.map((n) => n.nodeId));
            const cached = callsIndex.get(id);
            if (cached?.maxNodeId === stamp.maxNodeId) {
              toolCallNodes.push(...cached.nodes.filter((n) => !inWindow.has(n.nodeId)));
            } else {
              const callRows = sqlite
                .query<
                  {
                    node_id: number;
                    parent_node_id: number | null;
                    chat_message: string;
                    created_at: number;
                    metadata: string | null;
                  },
                  [string]
                >(
                  `select node_id, parent_node_id, chat_message, created_at, metadata
                 from message_nodes where session_id = ?
                   and json_array_length(chat_message, '$.tool_calls') > 0`,
                )
                .all(id);
              const parsed = parseRows(callRows.filter((r) => !inWindow.has(r.node_id))).filter(
                (n) => n.toolCalls.length > 0,
              );
              if (callsIndex.size >= SESSION_CACHE_MAX) callsIndex.clear();
              callsIndex.set(id, { maxNodeId: stamp.maxNodeId, nodes: parsed });
              toolCallNodes.push(...parsed);
            }
          }

          return Option.some<SessionNodeWindow>({
            nodes,
            toolCallNodes,
            total,
            start,
            backendType: stamp.backendType,
          });
        },
        catch: (error) =>
          new StorageError({ message: `Failed to page session nodes: ${String(error)}` }),
      });

    return SessionRepository.of({
      save,
      getById,
      list,
      delete: delete_,
      hasSession,
      nodesWindow,
    });
  });

/**
 * In-place conversation rewind for the Devin-format store: delete the
 * `message_nodes` rows the plan removed plus the `tool_call_state` /
 * `subagent_heads` rows that hung off them (guarded — a sepia-created
 * store may lack both tables), and move `last_activity_at`/`main_chain_id`
 * back to the surviving tail.
 *
 * Surviving rows are never rewritten — a `save` round-trip rebuilds every
 * `chat_message` blob through `Devin.buildChatMessage` (fresh message ids,
 * dropped store-specific fields), which is more damage than a rewind
 * should do to a store a live Devin install may also have open. Row
 * deletes are safe: FKs point at `sessions`, nothing references
 * `message_nodes`, and `node_id` is a contiguous per-session sequence, so
 * a suffix delete leaves no dangling `parent_node_id`.
 *
 * `prompt_history` is left alone — deliberately, on two grounds.
 * Schema: its rows are `(id, content, timestamp, session_id, is_shell)`,
 * so there is no `node_id`/`sequence_number` key to join the removed
 * nodes on. `timestamp` is the submit clock, not `created_at` (real
 * rows differ by hours), and `content` joins non-bijectively: devin
 * re-emits the same user message under fresh `node_id`s every time the
 * chain re-roots (one prompt-history row matches several nodes), so a
 * content delete would drop log entries whose prompts still live in
 * kept nodes. Semantics: devin itself only reads the table as
 * `SELECT content, is_shell ... ORDER BY timestamp, rowid` — it is the
 * input-recall log, not conversation state, which is also why
 * `Rewind.rewindSession` keeps `promptHistory` intact.
 *
 * `rendered_commits(session_id, sequence_number, rendered_html,
 * created_at)` is left for a harder reason: no join exists at all.
 * `message_nodes` carries no `sequence_number` — the V5 `message_forest`
 * migration dropped the whole `messages` table (the only sequence the
 * store ever had) when it introduced `node_id` — and `rendered_commits`
 * itself has no `node_id`/`message_id` column either. What
 * `sequence_number` counts is unverifiable from here: the CLI binary
 * contains only the table DDL and the session-delete cascade (no
 * INSERT/SELECT — it is written by the server-side renderer the DDL
 * comment calls "HTML strings for session restore"), real stores carry
 * zero rows, and the plausible readings
 * (forest `node_id`, main-chain position, monotonic commit counter) each
 * imply a different delete. Guessing wrong erases renders of surviving
 * states — worse than stale cache rows.
 */
export const truncateSessionNodes = (
  dbPath: string,
  sessionId: string,
  removal: {
    readonly removedNodeIds: ReadonlyArray<number>;
    readonly removedToolCallIds: ReadonlyArray<string>;
    readonly lastActivityAt: number;
    readonly mainChainId: number;
  },
): Effect.Effect<void, StorageError> =>
  Effect.try({
    try: () => {
      const sqlite = new Database(dbPath);
      try {
        sqlite.run("PRAGMA busy_timeout = 5000;");
        const tables = tableNames(sqlite);
        sqlite.transaction(() => {
          if (removal.removedNodeIds.length > 0) {
            const nodeMarks = removal.removedNodeIds.map(() => "?").join(", ");
            sqlite
              .query(`DELETE FROM message_nodes WHERE session_id = ? AND node_id IN (${nodeMarks})`)
              .run(sessionId, ...removal.removedNodeIds);
            if (tables.has("subagent_heads")) {
              // `chain_node_id` is the parent node that spawned the subagent —
              // a spawn that happened in a removed turn leaves a stale link.
              sqlite
                .query(
                  `DELETE FROM subagent_heads WHERE session_id = ? AND chain_node_id IN (${nodeMarks})`,
                )
                .run(sessionId, ...removal.removedNodeIds);
            }
          }
          if (tables.has("tool_call_state") && removal.removedToolCallIds.length > 0) {
            const callMarks = removal.removedToolCallIds.map(() => "?").join(", ");
            sqlite
              .query(
                `DELETE FROM tool_call_state WHERE session_id = ? AND tool_call_id IN (${callMarks})`,
              )
              .run(sessionId, ...removal.removedToolCallIds);
          }
          sqlite
            .query("UPDATE sessions SET last_activity_at = ?, main_chain_id = ? WHERE id = ?")
            .run(removal.lastActivityAt, removal.mainChainId, sessionId);
        })();
      } finally {
        sqlite.close();
      }
    },
    catch: (error) => new StorageError({ message: `Failed to truncate session: ${String(error)}` }),
  });

export const layer = (dbPath: string): Layer.Layer<SessionRepository, StorageError> =>
  Layer.effect(SessionRepository, make(dbPath));

/** Read-only layer: opens (and never migrates or writes) an existing store. */
export const layerReadonly = (dbPath: string): Layer.Layer<SessionRepository, StorageError> =>
  Layer.effect(SessionRepository, make(dbPath, { readonly: true }));
