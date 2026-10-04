import { useQuery } from "@tanstack/react-query";
import { useLiveQuery } from "@tanstack/react-db";
import { listSessions } from "../../lib/api";
import { sessionsCollection } from "../../lib/db";
import { queryKeys } from "./keys";

/**
 * Session rows come from the TanStack DB collection: optimistic mutations
 * land immediately and refetches that change nothing keep row (and list)
 * identity, so downstream `useMemo` work (filters, folder tree) skips
 * entirely. The paired `useQuery` shares the collection's `queryKey` — still
 * one fetch — and keeps the `error`/`isLoading` contract (e.g. AuthError →
 * token gate) that the collection's status flags can't express.
 */
export const useSessions = () => {
  const query = useQuery({ queryKey: queryKeys.sessions, queryFn: listSessions });
  // orderBy keeps the server's newest-first listing even as rows move —
  // collection insertion order alone wouldn't reflect updatedAt reorders.
  const live = useLiveQuery((q) =>
    q.from({ s: sessionsCollection }).orderBy(({ s }) => s.updatedAt, "desc"),
  );
  return {
    ...query,
    // Until the collection's first snapshot lands (or after a sync restart),
    // fall back to the query cache so consumers never see a false empty list.
    data: live.isReady ? live.data : query.data,
  };
};
