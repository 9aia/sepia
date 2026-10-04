import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getRecents, pushRecent } from "../lib/recents";

const store = new Map<string, string>();

const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
  clear: () => store.clear(),
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getRecents", () => {
  it("returns an empty list when nothing is stored", () => {
    expect(getRecents()).toEqual([]);
  });

  it("reads back stored ids", () => {
    store.set("sepia:recents", JSON.stringify(["a", "b"]));
    expect(getRecents()).toEqual(["a", "b"]);
  });

  it("degrades to empty on corrupt JSON", () => {
    store.set("sepia:recents", "{nope");
    expect(getRecents()).toEqual([]);
  });

  it("degrades to empty on a non-array payload", () => {
    store.set("sepia:recents", JSON.stringify({ ids: ["a"] }));
    expect(getRecents()).toEqual([]);
  });

  it("drops non-string entries", () => {
    store.set("sepia:recents", JSON.stringify(["a", 1, null, "b"]));
    expect(getRecents()).toEqual(["a", "b"]);
  });

  it("degrades to empty when storage itself throws", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      },
    });
    expect(getRecents()).toEqual([]);
  });
});

describe("pushRecent", () => {
  it("prepends the id, MRU first", () => {
    pushRecent("a");
    pushRecent("b");
    expect(getRecents()).toEqual(["b", "a"]);
  });

  it("dedupes an id back to the front", () => {
    pushRecent("a");
    pushRecent("b");
    pushRecent("a");
    expect(getRecents()).toEqual(["a", "b"]);
  });

  it("caps the list at 30 entries", () => {
    for (let i = 0; i < 35; i++) pushRecent(`s${i}`);
    const recents = getRecents();
    expect(recents).toHaveLength(30);
    expect(recents[0]).toBe("s34");
    expect(recents).not.toContain("s0");
  });

  it("silently does nothing when storage is unavailable", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => null,
      setItem: () => {
        throw new Error("denied");
      },
    });
    expect(() => pushRecent("a")).not.toThrow();
  });
});
