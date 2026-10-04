import { describe, expect, it, vi } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import { createQueryCollection } from "../lib/db-query-collection";

interface Row {
  id: string;
  v: number;
}

const rows = (...ids: Array<string>): Row[] => ids.map((id, v) => ({ id, v }));

const makeCollection = (
  client: QueryClient,
  queryFn: () => Promise<Row[]>,
  extra?: Partial<Parameters<typeof createQueryCollection<Row, string>>[0]>,
) =>
  createQueryCollection<Row, string>({
    id: "test-rows",
    queryClient: client,
    queryKey: ["test-rows"],
    queryFn,
    getKey: (row) => row.id,
    ...extra,
  });

describe("apply while optimistic mutation persists", () => {
  it("setQueryData insert during a persisting update still lands", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let resolvePatch: ((v: boolean) => void) | undefined;
    const collection = makeCollection(client, async () => rows("a", "b"), {
      onUpdate: async ({ transaction }) => {
        await new Promise<boolean>((resolve) => {
          resolvePatch = resolve;
        });
        client.setQueryData<Row[]>(["test-rows"], (old = []) =>
          old.map((r) => transaction.mutations.find((m) => m.key === r.id)?.modified ?? r),
        );
      },
    });
    await collection.preload();
    expect(collection.size).toBe(2);

    // Start a persisting optimistic update — never resolves until we say so.
    const tx = collection.update("a", (draft) => {
      draft.v = 99;
    });
    await vi.waitFor(() => expect(resolvePatch).toBeDefined());
    expect(collection.get("a")?.v).toBe(99); // optimistic

    // Now the create path: setQueryData adds a new row mid-persist.
    client.setQueryData<Row[]>(["test-rows"], (old = []) => [...old, { id: "c", v: 7 }]);

    // Optimistic overlay may defer the sync write; is it queued at least?
    console.log("after setQueryData: has c?", collection.has("c"), "state keys", [
      ...collection.state.keys(),
    ]);

    // Settle the optimistic mutation.
    resolvePatch?.(true);
    await tx.when("settled");
    await new Promise((r) => setTimeout(r, 0));
    console.log("after settle: has c?", collection.has("c"), "state keys", [
      ...collection.state.keys(),
    ]);
    expect(collection.has("c")).toBe(true);
  });

  it("setQueryData insert during a persisting update that FAILS still lands", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let rejectPatch: ((e: Error) => void) | undefined;
    const collection = makeCollection(client, async () => rows("a", "b"), {
      onUpdate: async () => {
        await new Promise<boolean>((_resolve, reject) => {
          rejectPatch = reject;
        });
      },
    });
    await collection.preload();

    const tx = collection.update("a", (draft) => {
      draft.v = 99;
    });
    await vi.waitFor(() => expect(rejectPatch).toBeDefined());

    client.setQueryData<Row[]>(["test-rows"], (old = []) => [...old, { id: "c", v: 7 }]);
    console.log("after setQueryData: has c?", collection.has("c"));

    rejectPatch?.(new Error("patch failed"));
    await expect(tx.when("settled")).rejects.toThrow();
    await new Promise((r) => setTimeout(r, 0));
    console.log("after rollback: has c?", collection.has("c"), "a.v:", collection.get("a")?.v);
    expect(collection.has("c")).toBe(true);
    expect(collection.get("a")?.v).toBe(0);
  });
});
