import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import { createQueryCollection } from "../lib/db-query-collection";

interface Row {
  id: string;
  v: number;
}

const rows = (...ids: Array<string>): Row[] => ids.map((id, v) => ({ id, v }));

const makeCollection = (client: QueryClient, queryFn: () => Promise<Row[]>, id = "test-rows") =>
  createQueryCollection<Row, string>({
    id,
    queryClient: client,
    queryKey: [id],
    queryFn,
    getKey: (row) => row.id,
  });

afterEach(() => {
  vi.useRealTimers();
});

describe("createQueryCollection resilience", () => {
  it("self-heals a transient fetch failure — no manual refetch needed", async () => {
    vi.useFakeTimers();
    // Fail the first load() cycle (all of the query's own retries), then
    // recover. The collection must keep retrying inside the sync run rather
    // than wedging in error state — core's startSync() cannot restart a sync
    // run that left `error`.
    let calls = 0;
    const client = new QueryClient();
    const collection = makeCollection(client, async () => {
      calls += 1;
      if (calls <= 4) throw new Error(`boom ${calls}`);
      return rows("a", "b");
    });

    const preload = collection.preload();
    // Query-level retries (1s + 2s + 4s) then the sync-level backoff (1s).
    await vi.advanceTimersByTimeAsync(60_000);
    await preload;

    expect(collection.status).toBe("ready");
    expect(collection.size).toBe(2);
    expect(calls).toBeGreaterThan(4);
  });

  it("surfaces error state on sustained failure, then recovers on the next success", async () => {
    vi.useFakeTimers();
    let fail = true;
    const client = new QueryClient();
    const collection = makeCollection(client, async () => {
      if (fail) throw new Error("server down");
      return rows("a");
    });

    const preload = collection.preload();
    const preloadSettled = expect(preload).rejects.toThrow();
    // 4+ failed load() cycles: (query retries ≈7s + sync backoff) each.
    await vi.advanceTimersByTimeAsync(120_000);
    await preloadSettled;
    expect(collection.status).toBe("error");

    // The sync run stays alive past markError — the next successful fetch
    // calls markReady, and error → ready is a valid transition.
    fail = false;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(collection.status).toBe("ready");
    expect(collection.size).toBe(1);
    expect(collection.get("a")?.v).toBe(0);
  });

  it("recovers through the cache subscription when the fetch never retries successfully", async () => {
    vi.useFakeTimers();
    let fail = true;
    const client = new QueryClient();
    const collection = makeCollection(client, async () => {
      if (fail) throw new Error("server down");
      return rows("x");
    });

    void collection.preload().catch(() => {});
    await vi.advanceTimersByTimeAsync(120_000);
    expect(collection.status).toBe("error");

    // An invalidateQueries/setQueryData-driven update reaches the collection
    // through the query-cache subscription — the same path the app's
    // useQuery fallback and mutation convergence use.
    fail = false;
    await client.refetchQueries({ queryKey: ["test-rows"], type: "all" });
    await vi.advanceTimersByTimeAsync(0);
    expect(collection.status).toBe("ready");
    expect(collection.size).toBe(1);
  });

  it("applies cached rows when the refetch fails but the query still holds data", async () => {
    vi.useFakeTimers();
    const client = new QueryClient();
    // Seed the cache, then make every fetch fail — the collection should
    // mirror the rows it already has instead of erroring on an empty screen.
    client.setQueryData<Row[]>(["test-rows"], rows("cached"));
    const collection = makeCollection(client, async () => {
      throw new Error("server down");
    });

    const preload = collection.preload();
    // The query's own retries have to exhaust before the rejection handler
    // falls back to the cached rows.
    await vi.advanceTimersByTimeAsync(30_000);
    await preload;
    expect(collection.status).toBe("ready");
    expect(collection.get("cached")?.id).toBe("cached");
  });

  it("stops retrying after teardown — no fetches past cleanup", async () => {
    vi.useFakeTimers();
    let calls = 0;
    const client = new QueryClient();
    const collection = makeCollection(client, async () => {
      calls += 1;
      throw new Error("server down");
    });

    void collection.preload().catch(() => {});
    await vi.advanceTimersByTimeAsync(30_000);
    const callsBeforeCleanup = calls;
    expect(callsBeforeCleanup).toBeGreaterThan(0);

    await collection.cleanup();
    // The in-flight fetchQuery keeps its own retryer after teardown (the
    // query cache still wants the result) — drain it, then prove the sync
    // run's retry timer is gone and nothing schedules further fetches.
    await vi.advanceTimersByTimeAsync(60_000);
    const callsAfterDrain = calls;
    await vi.advanceTimersByTimeAsync(120_000);
    expect(calls).toBe(callsAfterDrain);
    expect(collection.status).toBe("cleaned-up");
  });

  it("handles cleanup-during-sync then restart — a stale fetch can't wedge the new run", async () => {
    vi.useFakeTimers();
    // Simulates StrictMode double-mount: first sync run starts, the fetch is
    // in flight, teardown lands, a second sync run starts immediately after.
    let fail = true;
    const client = new QueryClient();
    const collection = makeCollection(client, async () => {
      if (fail) throw new Error("server down");
      return rows("ok");
    });

    void collection.preload().catch(() => {});
    await vi.advanceTimersByTimeAsync(0); // first fetch attempt in flight
    await collection.cleanup();
    expect(collection.status).toBe("cleaned-up");

    fail = false;
    const reload = collection.preload();
    // The new run's fetchQuery dedupes onto the still-pending fetch; its
    // retry backoff needs fake time to elapse.
    await vi.advanceTimersByTimeAsync(60_000);
    await reload;
    expect(collection.status).toBe("ready");
    expect(collection.size).toBe(1);

    // Let any stale retry timer/callback from the first run settle — the new
    // run must stay ready.
    await vi.advanceTimersByTimeAsync(120_000);
    expect(collection.status).toBe("ready");
  });
});
