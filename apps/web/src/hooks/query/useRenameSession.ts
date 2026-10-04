import { useMutation, useQueryClient } from "@tanstack/react-query";
import { renameSession } from "../../lib/api";
import { nodeTarget } from "../../lib/nodes";
import { queryKeys } from "./keys";

export const useRenameSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      title,
      agent,
      node,
    }: {
      id: string;
      title: string;
      agent?: string;
      node?: string;
    }) => renameSession(id, title, agent, nodeTarget(node)),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};
