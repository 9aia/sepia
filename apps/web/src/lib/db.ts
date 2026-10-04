import { deleteSession, patchSessionMeta, type SessionMetaPatch } from "./api";
import { createQueryCollection } from "./db-query-collection";
import { bareProjectId, sessionKey } from "./format";
import { listAllSessions, nodeTarget } from "./nodes";
import type { QueryClient } from "@tanstack/react-query";
import type { SessionSummary } from "./types";
import { queryKeys } from "../hooks/query/keys";
import { queryClient } from "../hooks/query/queryClient";

/**
 * Narrows a collection update's `changes` to the fields the session-meta
 * PATCH endpoint understands. `projectIds` arrive node-namespaced (merged
 * rows reference `node:id` keys); strip the prefix — the owning node's meta
 * overlay stores bare project ids.
 */
const metaPatchFrom = (changes: Partial<SessionSummary>): SessionMetaPatch => ({
  ...(changes.title !== undefined ? { title: changes.title } : {}),
  ...(changes.pinned !== undefined ? { pinned: changes.pinned } : {}),
  ...(changes.archived !== undefined ? { archived: changes.archived } : {}),
  ...(changes.projectIds !== undefined
    ? { projectIds: changes.projectIds.map(bareProjectId) }
    : {}),
  ...(changes.model !== undefined ? { model: changes.model } : {}),
});

/**
 * Sessions collection backed by the TanStack Query `["sessions"]` query — a
 * merged, per-node fan-out once peers are registered. The collection mirrors
 * the query cache under `queryKeys.sessions`, so `useQuery` consumers and
 * live-query views share one fetch and `invalidateQueries(["sessions"])`
 * refreshes both. `getKey` uses `node:agent:id` when `node` is set (local
 * rows carry the `"local"` segment) and falls back to `agent:id` — bare ids
 * collide across agents and would otherwise overwrite each other's rows.
 *
 * Mutations go through `onUpdate`/`onDelete`, which route the PATCH/DELETE to
 * the node that owns the row (`mutation.original.node`) and then converge
 * the query cache with `setQueryData` so the synced base matches the server
 * once the optimistic overlay releases — no extra fetch, no flash of the old
 * row. Throwing inside a handler rolls the optimistic change back per node.
 */
export const createSessionsCollection = (client: QueryClient) =>
  createQueryCollection<SessionSummary, string>({
    id: "sessions",
    queryClient: client,
    queryKey: queryKeys.sessions,
    queryFn: listAllSessions,
    getKey: (session) => sessionKey(session),
    onUpdate: async ({ transaction }) => {
      for (const mutation of transaction.mutations) {
        // `original` carries id + agent + node for the route; `changes` is
        // the diff produced by the optimistic draft.
        const ok = await patchSessionMeta(
          mutation.original.id,
          metaPatchFrom(mutation.changes),
          mutation.original.agent,
          nodeTarget(mutation.original.node),
        );
        if (!ok) throw new Error("The server rejected the session update");
      }
      const patched = new Map(transaction.mutations.map((m) => [m.key, m.modified]));
      client.setQueryData<SessionSummary[]>(queryKeys.sessions, (old) =>
        old === undefined ? old : old.map((s) => patched.get(sessionKey(s)) ?? s),
      );
    },
    onDelete: async ({ transaction }) => {
      for (const mutation of transaction.mutations) {
        await deleteSession(
          mutation.original.id,
          mutation.original.agent,
          nodeTarget(mutation.original.node),
        );
      }
      const deleted = new Set(transaction.mutations.map((m) => m.key));
      client.setQueryData<SessionSummary[]>(queryKeys.sessions, (old) =>
        old === undefined ? old : old.filter((s) => !deleted.has(sessionKey(s))),
      );
    },
  });

export type SessionsCollection = ReturnType<typeof createSessionsCollection>;

export const sessionsCollection = createSessionsCollection(queryClient);
