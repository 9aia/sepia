import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteSession } from "../../lib/api";
import { sepiaStore, setSelectedId } from "../../lib/store";
import { sessionsCollection } from "../../lib/db";
import { resolveSession, sessionKey } from "../../lib/format";
import { queryKeys } from "./keys";

export const useDeleteSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, agent }: { id: string; agent?: string }) => {
      // Ids collide across agents; `agent:id` is the collection key. A bare
      // id (old links) resolves by id only, like resolveSession elsewhere.
      const scoped = agent === undefined || agent === "" ? id : `${agent}:${id}`;
      const row = resolveSession([...sessionsCollection.state.values()], scoped);
      if (row === undefined) {
        // Not synced into the collection — e.g. a pending created-but-
        // unflushed session. The server still handles the delete; the row
        // never reaches the collection, so nothing rolls back client-side.
        await deleteSession(id, agent);
        return scoped;
      }
      const key = sessionKey(row);
      // Optimistic delete; the collection's onDelete issues the agent-scoped
      // DELETE and rolls the row back in on failure.
      await sessionsCollection.delete(key).when("settled");
      return key;
    },
    onSuccess: (key, { id, agent }) => {
      queryClient.removeQueries({ queryKey: queryKeys.history(id, agent) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // selectedId is the agent:id key; bare ids from old links match too.
      const selected = sepiaStore.state.selectedId;
      if (selected === key || selected === id) setSelectedId(null);
    },
  });
};
