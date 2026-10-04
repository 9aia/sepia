import type { Collection } from "@tanstack/db";
import type { QueryClient } from "@tanstack/react-query";
import { getConfig, setConfigKey } from "./api";
import { createQueryCollection } from "./db-query-collection";
import { queryKeys } from "../hooks/query/keys";
import { queryClient } from "../hooks/query/queryClient";

/** One item per config key so collection ops map to PATCH /api/config/:key. */
export interface ConfigItem {
  key: string;
  value: unknown;
}

/** GET /api/config, mapped from the `{config}` record into items. */
export const fetchConfigItems = async (): Promise<ConfigItem[]> => {
  const { config } = await getConfig();
  return Object.entries(config).map(([key, value]) => ({ key, value }));
};

/**
 * Config items as a Query-backed collection: reads mirror the
 * `queryKeys.config` query, `update`/`insert` apply optimistically and are
 * persisted by the collection's onUpdate/onInsert via `setConfigKey`. After a
 * successful PATCH the cached rows are updated too, so the synced base
 * converges to the written value when the optimistic overlay releases.
 */
export const createConfigCollection = (client: QueryClient): Collection<ConfigItem, string> => {
  const persist = async (mutations: ReadonlyArray<{ modified: ConfigItem }>): Promise<void> => {
    await Promise.all(mutations.map((m) => setConfigKey(m.modified.key, m.modified.value)));
    client.setQueryData<ConfigItem[]>(queryKeys.config, (old = []) => {
      const next = new Map(old.map((item) => [item.key, item]));
      for (const m of mutations) next.set(m.modified.key, m.modified);
      return [...next.values()];
    });
  };

  return createQueryCollection<ConfigItem, string>({
    id: "config",
    queryClient: client,
    queryKey: queryKeys.config,
    queryFn: fetchConfigItems,
    staleTime: 30_000,
    getKey: (item) => item.key,
    onInsert: ({ transaction }) => persist(transaction.mutations),
    onUpdate: ({ transaction }) => persist(transaction.mutations),
  });
};

/** App-wide config collection, bound to the shared queryClient. */
export const configCollection = createConfigCollection(queryClient);

/**
 * Optimistic upsert — the new value is visible to collection subscribers
 * immediately; the PATCH is awaited via `tx.when("settled")` (rejecting rolls
 * the optimistic write back). Only call once the collection is ready, so
 * pre-load defaults never overwrite saved server state.
 */
export const upsertConfigItem = (
  key: string,
  value: unknown,
  collection: Collection<ConfigItem, string> = configCollection,
) => {
  if (collection.has(key)) {
    return collection.update(key, (draft) => {
      draft.value = value;
    });
  }
  return collection.insert({ key, value });
};
