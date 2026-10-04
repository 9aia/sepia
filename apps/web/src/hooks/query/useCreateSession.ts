import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createSession } from "../../lib/api";
import type { CreateSessionInput } from "../../lib/types";
import { setSelectedId } from "../../lib/store";
import { queryKeys } from "./keys";

export const useCreateSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateSessionInput) => createSession(input),
    onSuccess: ({ id, agentId }) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // Selection keys are agent:id — the server echoes which agent spawned it.
      setSelectedId(agentId === undefined || agentId === "" ? id : `${agentId}:${id}`);
    },
  });
};
