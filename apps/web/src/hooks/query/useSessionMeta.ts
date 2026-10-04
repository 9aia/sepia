import { toastError, toastSuccess } from "../../lib/toast";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { patchSessionMeta, type SessionMetaPatch } from "../../lib/api";
import { sessionsCollection } from "../../lib/db";
import { bareProjectId, findSessionRow, sessionKey } from "../../lib/format";
import { nodeTarget } from "../../lib/nodes";
import { queryKeys } from "./keys";

const findRow = (id: string, agent?: string, node?: string) =>
  findSessionRow([...sessionsCollection.state.values()], { id, agent, node });

/** Merged rows carry node-namespaced project refs; the wire wants bare ids. */
const wirePatch = (patch: SessionMetaPatch): SessionMetaPatch => ({
  ...patch,
  ...(patch.projectIds !== undefined ? { projectIds: patch.projectIds.map(bareProjectId) } : {}),
});

/**
 * Optimistic session-meta writes: the draft applies to the collection
 * instantly, the collection's `onUpdate` handler routes the PATCH to the
 * owning node and converges the query cache, and a failure rolls the row
 * back to its last synced state. Rows not yet in the collection fall back to
 * a direct PATCH on their node.
 */
export const usePatchSessionMeta = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      id,
      agent,
      node,
      patch,
    }: {
      id: string;
      agent?: string;
      node?: string;
      patch: SessionMetaPatch;
    }) => {
      const row = findRow(id, agent, node);
      if (row === undefined) {
        // Not in the collection yet — e.g. a just-created session that only
        // exists as a server-side pending row until the next fetch. The API
        // can already patch it; the row syncs in on the refetch.
        if (!(await patchSessionMeta(id, wirePatch(patch), agent, nodeTarget(node)))) {
          throw new Error("The server rejected the session update");
        }
        await queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
        return;
      }
      const tx = sessionsCollection.update(sessionKey(row), (draft) => {
        // undefined entries aren't sent by JSON.stringify either — mirror
        // that so the optimistic row matches what the PATCH applies.
        for (const [key, value] of Object.entries(patch)) {
          if (value !== undefined) (draft as Record<string, unknown>)[key] = value;
        }
      });
      await tx.when("settled");
      if (patch.title !== undefined) toastSuccess("Session renamed");
      else if (patch.archived === true) toastSuccess("Session archived");
      else if (patch.archived === false) toastSuccess("Session unarchived");
      else if (patch.projectIds !== undefined) toastSuccess("Projects updated");
    },
    onError: (error) => toastError("Couldn't update the session", error),
  });
};
