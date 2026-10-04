import { useMutation, useQueryClient } from "@tanstack/react-query";
import { patchSessionMeta, type SessionMetaPatch } from "../../lib/api";
import { sessionsCollection } from "../../lib/db";
import { resolveSession, sessionKey } from "../../lib/format";
import { queryKeys } from "./keys";

const findRow = (id: string, agent?: string) => {
  const scoped = agent === undefined || agent === "" ? id : `${agent}:${id}`;
  return resolveSession([...sessionsCollection.state.values()], scoped);
};

/**
 * Optimistic session-meta writes: the draft applies to the collection
 * instantly, the collection's `onUpdate` handler PATCHes the server and
 * converges the query cache, and a failure rolls the row back to its last
 * synced state. Rows not yet in the collection fall back to a direct PATCH.
 */
export const usePatchSessionMeta = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      id,
      agent,
      patch,
    }: {
      id: string;
      agent?: string;
      patch: SessionMetaPatch;
    }) => {
      const row = findRow(id, agent);
      if (row === undefined) {
        // Not in the collection yet — e.g. a just-created session that only
        // exists as a server-side pending row until the next fetch. The API
        // can already patch it; the row syncs in on the refetch.
        if (!(await patchSessionMeta(id, patch, agent))) {
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
    },
  });
};
