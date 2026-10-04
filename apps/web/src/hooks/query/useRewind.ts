import { useMutation, useQueryClient } from "@tanstack/react-query";
import { rewindSession, type RewindSelector } from "../../lib/api";
import { nodeTarget } from "../../lib/nodes";
import type { RewindResult } from "../../lib/types";
import { queryKeys } from "./keys";

/**
 * Truncate the conversation at a point — a "Rewind to here" on a history
 * row passes `{nodeId}`, an undo-N-turns caller `{turns}`. The stored
 * transcript is rewritten on the owning node; history + session list +
 * checkpoints refetch so the truncated tail disappears everywhere.
 * The server detaches a live attach first — the next send re-attaches.
 */
export const useRewindSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      sessionId,
      agent,
      node,
      selector,
    }: {
      sessionId: string;
      agent?: string;
      node?: string;
      selector: RewindSelector;
    }): Promise<RewindResult> => rewindSession(sessionId, selector, agent, nodeTarget(node)),
    onSuccess: (_result, { sessionId, agent, node }) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.history(sessionId, agent, node) });
      void queryClient.invalidateQueries({
        queryKey: queryKeys.checkpoints(sessionId, agent, node),
      });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};

/** One-line summary a rewind result toast shows. */
export const rewindSummary = (result: RewindResult): string =>
  result.removed === 0
    ? "Nothing to rewind — already at that point"
    : `Rewound ${result.removed} message${result.removed === 1 ? "" : "s"}`;
