import { toastError, toastSuccess } from "../../lib/toast";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteSession } from "../../lib/api";
import { sepiaStore, setSelectedId } from "../../lib/store";
import { sessionsCollection } from "../../lib/db";
import { findSessionRow, sessionKey } from "../../lib/format";
import { nodeTarget } from "../../lib/nodes";
import { queryKeys } from "./keys";

export const useDeleteSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, agent, node }: { id: string; agent?: string; node?: string }) => {
      // Ids collide across agents and nodes; locate the row the way
      // resolveSession would — collection key is `node:agent:id` (or
      // `agent:id` single-node). Bare ids resolve on the local node.
      const row = findSessionRow([...sessionsCollection.state.values()], { id, agent, node });
      if (row === undefined) {
        // Not synced into the collection — e.g. a pending created-but-
        // unflushed session. The owning node still handles the delete; the
        // row never reaches the collection, so nothing rolls back here.
        await deleteSession(id, agent, nodeTarget(node));
        return id;
      }
      const key = sessionKey(row);
      // Optimistic delete; the collection's onDelete issues the routed
      // DELETE and rolls the row back in on failure.
      await sessionsCollection.delete(key).when("settled");
      return key;
    },
    onSuccess: (key, { id, agent, node }) => {
      queryClient.removeQueries({ queryKey: queryKeys.history(id, agent, node) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // selectedId is the scoped key; the legacy/bare forms match too.
      const selected = sepiaStore.state.selectedId;
      const legacy = agent === undefined || agent === "" ? id : `${agent}:${id}`;
      if (selected === key || selected === id || selected === legacy) setSelectedId(null);
      toastSuccess("Session deleted");
    },
    onError: (error) => toastError("Couldn't delete the session", error),
  });
};
