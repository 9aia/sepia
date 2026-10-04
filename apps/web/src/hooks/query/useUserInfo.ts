import { useQuery } from "@tanstack/react-query";
import { getUserInfo } from "../../lib/api";
import { queryKeys } from "./keys";

/** OS user/host info for the profile surfaces; static per server process. */
export const useUserInfo = () =>
  useQuery({
    queryKey: queryKeys.user,
    queryFn: getUserInfo,
    staleTime: Number.POSITIVE_INFINITY,
  });
