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
 * Consecutive fetch failures tolerated before the collection surfaces error
 * state. Each `load()` cycle already runs the query's own retries, so this
 * threshold sits on top of `retry: 3` — error state means a sustained outage
 * (server down through several backoff rounds), not a blip or a
 * StrictMode-cancelled retry. Recovery stays automatic either way: the sync
 * run keeps retrying and any cache update can still call markReady.
 */
const QUIET_FETCH_FAILURES = 3;

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
        // Set by the sync teardown — stops retry timers and stale fetch
        // callbacks from a StrictMode-style cleanup/restart racing the next
        // sync run (core also fences markReady/markError by sync epoch).
        let disposed = false;
        // Consecutive fetchQuery rejections in this sync run; drives the
        // retry backoff and the point at which error state is surfaced.
        let failures = 0;
        // markError already fired — later failures must not re-emit the
        // transition (each one re-notifies dependent live queries).
        let errored = false;
        let retryTimer: ReturnType<typeof setTimeout> | undefined;

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
              // markReady also recovers a collection that previously entered
              // error state (error → ready is a valid transition).
              errored = false;
              markReady();
            }
          } catch (error) {
            if (!ready) {
              // First snapshot never landed — surface it instead of leaving
              // preload() waiting forever. The cache subscription below stays
              // live, so a later successful update still recovers.
              errored = true;
              markError(error);
              return;
            }
            // A conflicting sync write races an in-flight optimistic
            // mutation — the optimistic overlay wins; the next cache update
            // reconciles the rows.
          }
        };

        // A rejected fetchQuery used to go straight to markError, which wedges
        // the collection: core's startSync() only restarts from idle/cleaned-up,
        // so subscribers could never trigger recovery. Instead keep the sync run
        // alive and retry with backoff; any success — ours, an invalidation, or
        // a setQueryData — lands in apply/markReady and clears the error state.
        const scheduleRetry = (error: unknown): void => {
          if (disposed) return;
          failures += 1;
          if (!ready && !errored && failures > QUIET_FETCH_FAILURES) {
            errored = true;
            markError(error);
          }
          retryTimer = setTimeout(
            () => {
              retryTimer = undefined;
              load();
            },
            Math.min(1000 * 2 ** (failures - 1), 10_000),
          );
        };

        const load = (): void => {
          void queryClient
            .fetchQuery({
              queryKey,
              queryFn,
              staleTime,
              retry: 3,
              retryDelay: (attempt) => Math.min(1000 * 2 ** attempt, 8000),
            })
            .then(
              (data) => {
                failures = 0;
                // fetchQuery resolves with the cached row set.
                if (Array.isArray(data)) apply(data as TItem[]);
              },
              (error: unknown) => {
                if (disposed) return;
                // The fetch failed but the cache may still hold rows — a
                // deduped observer fetch, an earlier load, or setQueryData.
                // Rows in hand beat error state; apply them and stand down.
                const cached = queryClient.getQueryData(queryKey);
                if (Array.isArray(cached)) {
                  failures = 0;
                  apply(cached as TItem[]);
                  return;
                }
                scheduleRetry(error);
              },
            );
        };

        const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
          if (event.type !== "updated") return;
          if (event.action.type !== "success" && event.action.type !== "setState") return;
          if (hashKey(event.query.queryKey) !== keyHash) return;
          const data = event.query.state.data;
          if (Array.isArray(data)) apply(data as TItem[]);
        });

        load();

        return () => {
          disposed = true;
          if (retryTimer !== undefined) clearTimeout(retryTimer);
          unsubscribe();
        };
      },
    },
  });
}
