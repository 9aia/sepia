import { useQuery } from "@tanstack/react-query";
import { listSessions } from "../../lib/api";
import { queryKeys } from "./keys";

export const useSessions = () => useQuery({ queryKey: queryKeys.sessions, queryFn: listSessions });
