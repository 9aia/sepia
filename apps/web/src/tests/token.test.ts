import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getToken, setToken } from "../lib/token";
import { setSettings } from "../lib/settings";

const store = new Map<string, string>();

const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
  setSettings({ localNodeUrl: null });
});

afterEach(() => {
  setSettings({ localNodeUrl: null });
  vi.unstubAllGlobals();
});

describe("token storage", () => {
  it("round-trips a token for the current local node address", () => {
    expect(getToken()).toBeNull();
    setToken("secret");
    expect(getToken()).toBe("secret");
    expect(store.get("sepia:token")).toBe('{"":"secret"}');
  });

  it("null and empty tokens clear the entry", () => {
    setToken("secret");
    setToken(null);
    expect(getToken()).toBeNull();
    setToken("secret");
    setToken("");
    expect(getToken()).toBeNull();
    expect(store.get("sepia:token")).toBeUndefined();
  });

  it("degrades gracefully when storage throws", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
      removeItem: () => {
        throw new Error("denied");
      },
    });
    expect(getToken()).toBeNull();
    expect(() => setToken("x")).not.toThrow();
    expect(() => setToken(null)).not.toThrow();
  });
});

describe("origin binding", () => {
  it("never sends a token to an address it wasn't entered for", () => {
    // Entered on the serving origin ("" slot).
    setToken("origin-token");
    // Repointing the local node must not carry the origin's credential.
    setSettings({ localNodeUrl: "https://other.example:8787" });
    expect(getToken()).toBeNull();
    // The other node gets its own token; both slots coexist.
    setToken("other-token");
    expect(getToken()).toBe("other-token");
    // Switching back restores the original credential — not the other node's.
    setSettings({ localNodeUrl: null });
    expect(getToken()).toBe("origin-token");
    expect(JSON.parse(store.get("sepia:token") ?? "{}")).toEqual({
      "": "origin-token",
      "https://other.example:8787": "other-token",
    });
  });

  it("clearing only drops the current address's slot", () => {
    setToken("origin-token");
    setSettings({ localNodeUrl: "https://other.example:8787" });
    setToken("other-token");
    setToken(null);
    expect(getToken()).toBeNull();
    setSettings({ localNodeUrl: null });
    expect(getToken()).toBe("origin-token");
  });

  it("migrates a legacy bare-string store into the current address's slot", () => {
    store.set("sepia:token", "legacy");
    expect(getToken()).toBe("legacy");
    // And a repoint still can't carry it away.
    setSettings({ localNodeUrl: "https://other.example:8787" });
    expect(getToken()).toBeNull();
  });
});
