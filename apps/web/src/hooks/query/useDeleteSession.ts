import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteSession } from "../../lib/api";
import { sepiaStore, setSelectedId } from "../../lib/store";
import { queryKeys } from "./keys";

export const useDeleteSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteSession(id),
    onSuccess: (_ok, id) => {
      queryClient.removeQueries({ queryKey: queryKeys.history(id) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      if (sepiaStore.state.selectedId === id) setSelectedId(null);
    },
  });
};
