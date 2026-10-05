import { useQuery } from "@tanstack/react-query";
import { AuthError, getUserInfo } from "../../lib/api";
import { queryKeys } from "./keys";

/**
 * OS user/host info for the profile surfaces; static per server process.
 * The profile is local-node data — it can only fail when that node is
 * unreachable (then the profile is simply absent, no error surface) or
 * unauthorized (the TokenGate owns that case). Non-auth failures resolve to
 * "no profile"; a 401 still errors so auth semantics stay visible.
 */
export const useUserInfo = () =>
  useQuery({
    queryKey: queryKeys.user,
    queryFn: async () => {
      try {
        return await getUserInfo();
      } catch (error) {
        if (error instanceof AuthError) throw error;
        return undefined;
      }
    },
    staleTime: Number.POSITIVE_INFINITY,
  });
