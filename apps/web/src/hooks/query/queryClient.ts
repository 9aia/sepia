import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Server state is local and cheap; a short stale window avoids
      // refetch storms between components without hiding live updates.
      staleTime: 10_000,
      retry: 1,
    },
  },
});
