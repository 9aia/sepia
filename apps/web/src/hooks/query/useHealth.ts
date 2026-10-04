import { useQuery } from "@tanstack/react-query";

/**
 * Server reachability — pings /api/health (unauthenticated). `isError` means
 * the API is down or unreachable; drives the offline indicator + empty state.
 */
export const useHealth = () =>
  useQuery({
    queryKey: ["health"],
    queryFn: async (): Promise<{ status: string }> => {
      const res = await fetch("/api/health");
      if (!res.ok) throw new Error(`health ${res.status}`);
      return res.json() as Promise<{ status: string }>;
    },
    refetchInterval: 10_000,
    refetchIntervalInBackground: true,
    retry: 1,
    staleTime: 0,
  });
