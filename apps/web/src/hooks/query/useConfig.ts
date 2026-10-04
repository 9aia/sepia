import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { getConfig, setConfigKey } from "../../lib/api";
import { queryKeys } from "./keys";

export const useConfig = () =>
  useQuery({
    queryKey: queryKeys.config,
    queryFn: getConfig,
    staleTime: 30_000,
    select: (data) => data.config,
  });

export const useSetConfig = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ key, value }: { key: string; value: unknown }) => setConfigKey(key, value),
    onMutate: ({ key, value }) => {
      // Optimistic write — the UI state should apply immediately.
      const prev = queryClient.getQueryData<{ config: Record<string, unknown> }>(queryKeys.config);
      if (prev !== undefined) {
        queryClient.setQueryData(queryKeys.config, {
          config: { ...prev.config, [key]: value },
        });
      }
      return { prev };
    },
    onError: (_err, _vars, ctx) => {
      if (ctx?.prev !== undefined) queryClient.setQueryData(queryKeys.config, ctx.prev);
    },
    onSettled: () => void queryClient.invalidateQueries({ queryKey: queryKeys.config }),
  });
};

/**
 * Server-persisted UI state — local useState seeded from /api/config once it
 * loads (avoids a collapse flicker), write-through to PATCH /api/config/:key.
 * Until config loads, changes stay local so nothing wrong gets persisted.
 */
export const useUiState = <T>(key: string, def: T): [T, (next: T | ((prev: T) => T)) => void] => {
  const { data: config } = useConfig();
  const setConfig = useSetConfig();
  const [value, setValue] = useState(def);
  const loaded = useRef(false);
  useEffect(() => {
    if (!loaded.current && config !== undefined) {
      loaded.current = true;
      const saved = config[key];
      if (saved !== undefined) setValue(saved as T);
    }
  }, [config, key]);
  const set = (next: T | ((prev: T) => T)): void => {
    const resolved = typeof next === "function" ? (next as (p: T) => T)(value) : next;
    setValue(resolved);
    if (loaded.current) setConfig.mutate({ key, value: resolved });
  };
  return [value, set];
};
