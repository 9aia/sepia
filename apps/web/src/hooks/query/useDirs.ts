import { useQuery } from "@tanstack/react-query";
import { listDirs } from "../../lib/api";
import { queryKeys } from "./keys";

/** Subdirectories of an absolute path, for the cwd field's datalist. */
export const useDirs = (path: string | null) =>
  useQuery({
    queryKey: queryKeys.dirs(path ?? ""),
    queryFn: () => listDirs(path ?? "/"),
    enabled: path !== null,
    staleTime: 30_000,
    retry: 1,
  });
