import { useMutation, useQueryClient } from "@tanstack/react-query";
import { renameSession } from "../../lib/api";
import { queryKeys } from "./keys";

export const useRenameSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, title }: { id: string; title: string }) => renameSession(id, title),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};
