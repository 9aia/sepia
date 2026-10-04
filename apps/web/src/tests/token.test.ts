import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { getToken, setToken } from "../lib/token";

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
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("token storage", () => {
  it("round-trips a token", () => {
    expect(getToken()).toBeNull();
    setToken("secret");
    expect(getToken()).toBe("secret");
    expect(store.get("sepia:token")).toBe("secret");
  });

  it("null and empty tokens clear the entry", () => {
    setToken("secret");
    setToken(null);
    expect(getToken()).toBeNull();
    setToken("secret");
    setToken("");
    expect(getToken()).toBeNull();
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
