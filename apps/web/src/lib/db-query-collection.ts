import {
  createCollection,
  deepEquals,
  type Collection,
  type DeleteMutationFn,
  type InsertMutationFn,
  type UpdateMutationFn,
} from "@tanstack/db";
import { hashKey, type QueryClient, type QueryKey } from "@tanstack/react-query";

/**
 * Minimal Query-backed collection — fills the role of
 * `@tanstack/query-db-collection` (not installed). A React Query query owns
 * fetching and caching under `queryKey`; the collection mirrors the cached
 * rows through a query-cache subscription, so `fetchQuery`, `setQueryData`,
 * and `invalidateQueries`-driven refetches all converge into the collection's
 * synced state. `onInsert`/`onUpdate`/`onDelete` persist optimistic writes.
 */
export interface QueryCollectionOptions<TItem extends object, TKey extends string | number> {
  /** Stable id for the collection (debugging + GC bookkeeping). */
  id: string;
  /** The QueryClient that owns `queryKey`. */
  queryClient: QueryClient;
  /** Query key whose cached `TItem[]` the collection mirrors. */
  queryKey: QueryKey;
  /** Fetches the full row set; the resolved array is cached under `queryKey`. */
  queryFn: () => Promise<TItem[]>;
  getKey: (item: TItem) => TKey;
  /** Stale window for the underlying query's `fetchQuery`. */
  staleTime?: number;
  /** Start syncing immediately instead of on the first subscriber. */
  startSync?: boolean;
  onInsert?: InsertMutationFn<TItem, TKey>;
  onUpdate?: UpdateMutationFn<TItem, TKey>;
  onDelete?: DeleteMutationFn<TItem, TKey>;
}

export function createQueryCollection<TItem extends object, TKey extends string | number = string>(
  options: QueryCollectionOptions<TItem, TKey>,
): Collection<TItem, TKey> {
  const { queryClient, queryKey, queryFn, staleTime } = options;
  const keyHash = hashKey(queryKey);

  return createCollection<TItem, TKey>({
    id: options.id,
    getKey: options.getKey,
    startSync: options.startSync,
    onInsert: options.onInsert,
    onUpdate: options.onUpdate,
    onDelete: options.onDelete,
    sync: {
      sync: ({ begin, write, commit, markReady, markError }) => {
        // Rows last applied from the query cache — diffed per cache update so
        // unchanged rows don't emit writes and optimistic overlays stay put.
        let seen = new Map<TKey, TItem>();
        let ready = false;

        const apply = (items: ReadonlyArray<TItem>): void => {
          const next = new Map(items.map((item) => [options.getKey(item), item]));
          try {
            begin();
            for (const [key, item] of next) {
              const prev = seen.get(key);
              if (prev === undefined) {
                write({ type: "insert", value: item });
              } else if (!deepEquals(prev, item)) {
                write({ type: "update", value: item });
              }
            }
            for (const key of seen.keys()) {
              if (!next.has(key)) write({ type: "delete", key });
            }
            const receipt = commit();
            if (receipt !== true) {
              // Sync writes can queue behind persisting optimistic mutations;
              // a cancelled receipt just means a later apply reconciles.
              void receipt.catch(() => {});
            }
            seen = next;
            if (!ready) {
              ready = true;
              markReady();
            }
          } catch (error) {
            if (!ready) {
              // First snapshot never landed — surface it instead of leaving
              // preload() waiting forever.
              markError(error);
              return;
            }
            // A conflicting sync write races an in-flight optimistic
            // mutation — the optimistic overlay wins; the next cache update
            // reconciles the rows.
          }
        };

        const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
          if (event.type !== "updated") return;
          if (event.action.type !== "success" && event.action.type !== "setState") return;
          if (hashKey(event.query.queryKey) !== keyHash) return;
          const data = event.query.state.data;
          if (Array.isArray(data)) apply(data as TItem[]);
        });

        void queryClient.fetchQuery({ queryKey, queryFn, staleTime }).then(apply).catch(markError);

        return () => unsubscribe();
      },
    },
  });
}
