import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteSession } from "../../lib/api";
import { sepiaStore, setSelectedId } from "../../lib/store";
import { resolveSession, sessionKey } from "../../lib/format";
import { queryKeys } from "./keys";

export const useDeleteSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, agent }: { id: string; agent?: string }) => deleteSession(id, agent),
    onSuccess: (_ok, { id, agent }) => {
      queryClient.removeQueries({ queryKey: queryKeys.history(id, agent) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // selectedId is the agent:id key — compare against the deleted session's key.
      const scoped = agent === undefined || agent === "" ? id : `${agent}:${id}`;
      const qc = queryClient;
      const sessions = qc.getQueryData<{ sessions?: Array<{ agent: string; id: string }> }>(
        queryKeys.sessions,
      )?.sessions;
      const match = sessions !== undefined ? resolveSession(sessions, scoped) : undefined;
      const key = match !== undefined ? sessionKey(match) : scoped;
      if (sepiaStore.state.selectedId === key) setSelectedId(null);
    },
  });
};
