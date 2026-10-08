import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getRecents, pushRecent, resolveRecentSessions } from "../lib/recents";

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

describe("resolveRecentSessions", () => {
  const session = (id: string, agent = "devin", node?: string) => ({
    id,
    agent,
    ...(node === undefined ? {} : { node }),
  });

  it("keeps the input list's order, not open recency", () => {
    // "Sessions" section order follows the list (updatedAt/sort pick) —
    // MRU order would move the clicked row to the top on every select.
    const sessions = [session("a"), session("b"), session("c")];
    const recents = ["devin:c", "devin:a"];
    expect(resolveRecentSessions(sessions, recents).map((s) => s.id)).toEqual(["a", "c"]);
  });

  it("does not reorder when a session is opened again", () => {
    const sessions = [session("a"), session("b"), session("c")];
    pushRecent("devin:a");
    pushRecent("devin:c");
    const before = resolveRecentSessions(sessions).map((s) => s.id);
    // Re-selecting "c" bumps it to the MRU front — the section must not move.
    pushRecent("devin:c");
    const after = resolveRecentSessions(sessions).map((s) => s.id);
    expect(after).toEqual(before);
    expect(after).toEqual(["a", "c"]);
  });

  it("drops keys that resolve to nothing", () => {
    const sessions = [session("a")];
    expect(
      resolveRecentSessions(sessions, ["devin:ghost", "devin:a", "gone"]).map((s) => s.id),
    ).toEqual(["a"]);
  });

  it("dedupes rows reached through bare and scoped keys", () => {
    const sessions = [session("a"), session("a", "cline", "node_x")];
    // Bare "a" resolves to the local row first; the scoped key hits the
    // same row — one entry, not two.
    expect(resolveRecentSessions(sessions, ["a", "devin:a"]).map((s) => s.agent)).toEqual([
      "devin",
    ]);
    expect(
      resolveRecentSessions(sessions, ["a", "devin:a", "node_x:cline:a"]).map((s) => s.agent),
    ).toEqual(["devin", "cline"]);
  });
});
