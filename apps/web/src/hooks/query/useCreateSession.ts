import { toastError } from "../../lib/toast";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createSession } from "../../lib/api";
import { LOCAL_NODE_ID } from "../../lib/format";
import { isMultiNode, nodeTarget } from "../../lib/nodes";
import type { CreateSessionInput } from "../../lib/types";
import { setSelectedId } from "../../lib/store";
import { queryKeys } from "./keys";

export const useCreateSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ node, ...input }: CreateSessionInput & { node?: string }) =>
      createSession(input, nodeTarget(node)),
    onError: (error) => toastError("Couldn't create the session", error),
    onSuccess: ({ id, agentId }, { node }) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      // Selection keys are node:agent:id when the create targeted a
      // registered node, else the legacy agent:id (resolves to the local
      // node either way — but the explicit segment keeps it correct if a
      // peer shares the id).
      if (agentId === undefined || agentId === "") {
        setSelectedId(id);
        return;
      }
      const segment = node !== undefined ? node : isMultiNode() ? LOCAL_NODE_ID : undefined;
      setSelectedId(segment === undefined ? `${agentId}:${id}` : `${segment}:${agentId}:${id}`);
    },
  });
};
