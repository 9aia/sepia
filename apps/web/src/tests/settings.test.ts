import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import type { SepiaSettings } from "../lib/settings";

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

const DEFAULTS: SepiaSettings = {
  defaultAgent: null,
  defaultCwd: null,
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
};

/** Re-import the module fresh so its store re-runs load() against `store`. */
const loadSettings = async (): Promise<SepiaSettings> => {
  vi.resetModules();
  const mod = await import("../lib/settings");
  return mod.settingsStore.state;
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("settings load", () => {
  it("defaults when nothing is stored", async () => {
    expect(await loadSettings()).toEqual(DEFAULTS);
  });

  it("defaults when the stored payload is corrupt", async () => {
    store.set("sepia:settings", "{not json");
    expect(await loadSettings()).toEqual(DEFAULTS);
  });

  it("coerces each field independently — bad types fall back", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        defaultAgent: 42,
        defaultCwd: "/work",
        models: "nope",
        keybinds: "no",
        notifications: "no",
        theme: "purple",
      }),
    );
    const loaded = await loadSettings();
    expect(loaded.defaultAgent).toBeNull();
    expect(loaded.defaultCwd).toBe("/work");
    expect(loaded.models).toEqual({});
    expect(loaded.keybinds).toEqual({});
    expect(loaded.notifications).toEqual({ enabled: false, done: true, permission: true });
    expect(loaded.theme).toBe("dark");
  });

  it("accepts light and system themes", async () => {
    store.set("sepia:settings", JSON.stringify({ theme: "light" }));
    expect((await loadSettings()).theme).toBe("light");
    store.set("sepia:settings", JSON.stringify({ theme: "system" }));
    expect((await loadSettings()).theme).toBe("system");
  });

  it("notifications default per-field: only explicit false flips done/permission", async () => {
    store.set("sepia:settings", JSON.stringify({ notifications: { enabled: true } }));
    expect((await loadSettings()).notifications).toEqual({
      enabled: true,
      done: true,
      permission: true,
    });
    store.set(
      "sepia:settings",
      JSON.stringify({ notifications: { done: false, permission: false } }),
    );
    expect((await loadSettings()).notifications).toEqual({
      enabled: false,
      done: false,
      permission: false,
    });
  });
});

describe("setSettings", () => {
  it("merges the patch and persists the whole settings object", async () => {
    vi.resetModules();
    const mod = await import("../lib/settings");
    mod.setSettings({ theme: "light", defaultAgent: "cline" });
    expect(mod.settingsStore.state.theme).toBe("light");
    expect(mod.settingsStore.state.defaultAgent).toBe("cline");
    // Untouched keys keep their values.
    expect(mod.settingsStore.state.notifications).toEqual(DEFAULTS.notifications);

    const persisted = JSON.parse(store.get("sepia:settings") ?? "{}") as SepiaSettings;
    expect(persisted.theme).toBe("light");
    expect(persisted.defaultAgent).toBe("cline");
  });

  it("keeps working when persistence throws", async () => {
    vi.resetModules();
    const mod = await import("../lib/settings");
    vi.stubGlobal("localStorage", {
      getItem: () => null,
      setItem: () => {
        throw new Error("denied");
      },
    });
    expect(() => mod.setSettings({ theme: "system" })).not.toThrow();
    expect(mod.settingsStore.state.theme).toBe("system");
  });
});
