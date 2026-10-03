import { useQuery } from "@tanstack/react-query";
import { listAgents } from "../../lib/api";
import { queryKeys } from "./keys";

/** The agent roster changes only with server config, so it never goes stale. */
export const useAgents = () =>
  useQuery({
    queryKey: queryKeys.agents,
    queryFn: listAgents,
    staleTime: Number.POSITIVE_INFINITY,
  });
