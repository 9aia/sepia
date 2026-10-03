import { useQuery } from "@tanstack/react-query";
import { getHistory } from "../../lib/api";
import { queryKeys } from "./keys";

/**
 * Stored backlog for a session. The live SSE feed invalidates this key when a
 * run finishes so the pane re-syncs without polling.
 */
export const useHistory = (sessionId: string | null) =>
  useQuery({
    queryKey: queryKeys.history(sessionId ?? ""),
    queryFn: () => getHistory(sessionId as string),
    enabled: sessionId !== null,
  });
