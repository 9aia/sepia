import { useMutation, useQueryClient } from "@tanstack/react-query";
import { renameSession } from "../../lib/api";
import { queryKeys } from "./keys";

export const useRenameSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, title, agent }: { id: string; title: string; agent?: string }) =>
      renameSession(id, title, agent),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};
