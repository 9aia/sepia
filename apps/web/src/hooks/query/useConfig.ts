import { eq } from "@tanstack/db";
import { useMutation } from "@tanstack/react-query";
import { useLiveQuery } from "@tanstack/react-db";
import { useEffect, useRef, useState } from "react";
import { configCollection, upsertConfigItem } from "../../lib/db-config";

/**
 * The server config record, backed by `configCollection` (which mirrors the
 * `queryKeys.config` query). `data` is `Record<string, unknown>` and stays
 * `undefined` until the collection is ready — same shape as before.
 */
export const useConfig = () => {
  const { data, isLoading, isReady, status } = useLiveQuery((q) =>
    q.from({ item: configCollection }),
  );
  const config =
    data === undefined ? undefined : Object.fromEntries(data.map((item) => [item.key, item.value]));
  return {
    data: isReady ? config : undefined,
    isLoading,
    isReady,
    status,
  };
};

/**
 * Write-through PATCH /api/config/:key. `upsertConfigItem` applies the change
 * optimistically in `configCollection` and the collection's onInsert/onUpdate
 * persists it — a failed write rolls the optimistic change back and surfaces
 * as a mutation error.
 */
export const useSetConfig = () =>
  useMutation({
    mutationFn: async ({ key, value }: { key: string; value: unknown }) => {
      const tx = upsertConfigItem(key, value);
      await tx.when("settled");
    },
  });

/**
 * Server-persisted UI state — local useState seeded from the config
 * collection once it is ready (avoids a collapse flicker), write-through to
 * PATCH /api/config/:key via an optimistic collection upsert. Until the
 * collection is ready, changes stay local so nothing wrong gets persisted.
 */
export const useUiState = <T>(key: string, def: T): [T, (next: T | ((prev: T) => T)) => void] => {
  const { data: item, isReady } = useLiveQuery((q) =>
    q
      .from({ item: configCollection })
      .where(({ item }) => eq(item.key, key))
      .findOne(),
  );
  const [value, setValue] = useState(def);
  const loaded = useRef(false);
  useEffect(() => {
    if (!loaded.current && isReady) {
      loaded.current = true;
      const saved = item?.value;
      if (saved !== undefined) setValue(saved as T);
    }
  }, [isReady, item]);
  const set = (next: T | ((prev: T) => T)): void => {
    const resolved = typeof next === "function" ? (next as (p: T) => T)(value) : next;
    setValue(resolved);
    if (loaded.current) {
      const tx = upsertConfigItem(key, resolved);
      // Persist failures roll the optimistic write back; the local value
      // stays (same as before, where the seed-out state survived errors).
      void tx.when("settled").catch(() => {});
    }
  };
  return [value, set];
};
