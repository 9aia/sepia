import { QueryClient } from "@tanstack/react-query";
import { AuthError } from "../../lib/api";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Server state is local and cheap; a short stale window avoids
      // refetch storms between components without hiding live updates.
      staleTime: 10_000,
      // A 401 can't fix itself — retrying only delays the TokenGate and
      // produces the gate → loading → gate flicker while refetches cycle.
      retry: (failureCount, error) => !(error instanceof AuthError) && failureCount < 1,
    },
  },
});
