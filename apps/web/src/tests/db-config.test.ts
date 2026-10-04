import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getConfig, setConfigKey } from "../lib/api";
import { createConfigCollection, upsertConfigItem, type ConfigItem } from "../lib/db-config";
import { queryKeys } from "../hooks/query/keys";

vi.mock("../lib/api", () => ({
  getConfig: vi.fn(),
  setConfigKey: vi.fn(),
}));

const mockedGetConfig = vi.mocked(getConfig);
const mockedSetConfigKey = vi.mocked(setConfigKey);

describe("configCollection", () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    vi.clearAllMocks();
    queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    mockedSetConfigKey.mockResolvedValue(undefined);
  });

  it("is not ready before the first load (the useUiState loaded gate)", async () => {
    mockedGetConfig.mockResolvedValue({ config: { a: 1 } });
    const collection = createConfigCollection(queryClient);
    expect(collection.isReady()).toBe(false);
    await collection.preload();
    expect(collection.isReady()).toBe(true);
  });

  it("syncs getConfig into one item per key", async () => {
    mockedGetConfig.mockResolvedValue({
      config: { "ui.section.folders": false, n: 1 },
    });
    const collection = createConfigCollection(queryClient);
    await collection.preload();
    expect(collection.get("ui.section.folders")?.value).toBe(false);
    expect(collection.get("n")?.value).toBe(1);
  });

  it("write-through: update applies optimistically and persists via setConfigKey", async () => {
    mockedGetConfig.mockResolvedValue({ config: { a: 1 } });
    const collection = createConfigCollection(queryClient);
    await collection.preload();

    const tx = upsertConfigItem("a", 2, collection);
    // Optimistic — visible before the PATCH resolves.
    expect(collection.get("a")?.value).toBe(2);
    expect(mockedSetConfigKey).toHaveBeenCalledWith("a", 2);
    await tx.when("settled");
    expect(collection.get("a")?.value).toBe(2);
  });

  it("write-through: a new key inserts instead of updating", async () => {
    mockedGetConfig.mockResolvedValue({ config: {} });
    const collection = createConfigCollection(queryClient);
    await collection.preload();

    const tx = upsertConfigItem("ui.new", [1, 2], collection);
    expect(collection.get("ui.new")?.value).toEqual([1, 2]);
    await tx.when("settled");
    expect(mockedSetConfigKey).toHaveBeenCalledWith("ui.new", [1, 2]);
  });

  it("rolls an optimistic insert back when the PATCH fails", async () => {
    mockedGetConfig.mockResolvedValue({ config: {} });
    const collection = createConfigCollection(queryClient);
    await collection.preload();

    mockedSetConfigKey.mockRejectedValue(new Error("nope"));
    const tx = upsertConfigItem("bad", true, collection);
    expect(collection.has("bad")).toBe(true);
    await expect(tx.when("settled")).rejects.toThrow("nope");
    expect(collection.has("bad")).toBe(false);
  });

  it("mirrors query-cache updates (setQueryData) into the collection", async () => {
    mockedGetConfig.mockResolvedValue({ config: { a: 1 } });
    const collection = createConfigCollection(queryClient);
    await collection.preload();

    queryClient.setQueryData<ConfigItem[]>(queryKeys.config, [
      { key: "a", value: 10 },
      { key: "b", value: 2 },
    ]);
    expect(collection.get("a")?.value).toBe(10);
    expect(collection.get("b")?.value).toBe(2);
  });

  it("drops keys that disappear from a refetch", async () => {
    mockedGetConfig.mockResolvedValue({ config: { a: 1, gone: true } });
    const collection = createConfigCollection(queryClient);
    await collection.preload();
    expect(collection.has("gone")).toBe(true);

    queryClient.setQueryData<ConfigItem[]>(queryKeys.config, [{ key: "a", value: 1 }]);
    expect(collection.has("gone")).toBe(false);
    expect(collection.get("a")?.value).toBe(1);
  });
});
