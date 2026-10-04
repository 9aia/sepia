import { useInfiniteQuery, type InfiniteData } from "@tanstack/react-query";
import { getHistory } from "../../lib/api";
import type { HistoryMessage, HistoryPage } from "../../lib/types";
import { queryKeys } from "./keys";

const PAGE_SIZE = 200;

/** Pages are newest-first in the cache; flatten oldest → newest. */
export const flattenHistory = (data: InfiniteData<HistoryPage> | undefined): HistoryMessage[] =>
  data === undefined
    ? []
    : data.pages
        .slice()
        .reverse()
        .flatMap((page) => page.messages)
        // Agents persist their system prompt as IR nodes (devin even writes
        // <system_info> twice) — they're context, not conversation.
        .filter((message) => message.role !== "system");

/**
 * Stored backlog for a session, paged backwards via the `start` cursor. The
 * live SSE feed invalidates this key when a run finishes so the pane re-syncs
 * without polling.
 */
export const useHistory = (sessionId: string | null, agent?: string) =>
  useInfiniteQuery({
    queryKey: queryKeys.history(sessionId ?? "", agent),
    queryFn: ({ pageParam }) =>
      getHistory(sessionId as string, {
        limit: PAGE_SIZE,
        before: pageParam === 0 ? undefined : pageParam,
        agent,
      }),
    enabled: sessionId !== null,
    initialPageParam: 0,
    getNextPageParam: (last) => (last.start > 0 ? last.start : undefined),
  });
