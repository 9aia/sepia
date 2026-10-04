import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteSession } from "../../lib/api";
import { sepiaStore, setSelectedId } from "../../lib/store";
import { resolveSession, sessionKey } from "../../lib/format";
import { queryKeys } from "./keys";

export const useDeleteSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteSession(id),
    onSuccess: (_ok, id) => {
      queryClient.removeQueries({ queryKey: queryKeys.history(id) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // selectedId is the agent:id key — compare against the deleted session's key.
      const qc = queryClient;
      const sessions = qc.getQueryData<{ sessions?: Array<{ agent: string; id: string }> }>(
        queryKeys.sessions,
      )?.sessions;
      const match = sessions !== undefined ? resolveSession(sessions, id) : undefined;
      const key = match !== undefined ? sessionKey(match) : id;
      if (sepiaStore.state.selectedId === key) setSelectedId(null);
    },
  });
};
