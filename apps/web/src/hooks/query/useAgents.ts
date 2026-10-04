import { useQuery } from "@tanstack/react-query";
import { listAllAgents } from "../../lib/nodes";
import { queryKeys } from "./keys";

/** The agent roster changes only with server config, so it never goes stale. */
export const useAgents = () =>
  useQuery({
    queryKey: queryKeys.agents,
    queryFn: listAllAgents,
    staleTime: Number.POSITIVE_INFINITY,
  });
