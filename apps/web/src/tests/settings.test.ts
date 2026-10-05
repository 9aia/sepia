import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  defaultAgentFor,
  defaultCwdFor,
  recentCwdFor,
  withNodeDefault,
  type SepiaSettings,
} from "../lib/settings";
import { defaultSidebarSections } from "../lib/sidebar";

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
  defaultAgent: {},
  defaultCwd: {},
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  localNodeEnabled: true,
  sidebar: { sections: defaultSidebarSections() },
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
    expect(loaded.defaultAgent).toEqual({});
    // A stored scalar cwd migrates onto the local node key.
    expect(loaded.defaultCwd).toEqual({ local: "/work" });
    expect(loaded.models).toEqual({});
    expect(loaded.keybinds).toEqual({});
    expect(loaded.notifications).toEqual({ enabled: false, done: true, permission: true });
    expect(loaded.theme).toBe("dark");
  });

  it("migrates a stored scalar defaultAgent onto the local node key", async () => {
    store.set("sepia:settings", JSON.stringify({ defaultAgent: "cline" }));
    expect((await loadSettings()).defaultAgent).toEqual({ local: "cline" });
  });

  it("keeps node-keyed default maps, dropping non-string entries", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        defaultAgent: { local: "devin", node_a1b2: "cline", bad: 7, empty: "" },
        defaultCwd: { node_a1b2: "/peer/work" },
      }),
    );
    const loaded = await loadSettings();
    expect(loaded.defaultAgent).toEqual({ local: "devin", node_a1b2: "cline" });
    expect(loaded.defaultCwd).toEqual({ node_a1b2: "/peer/work" });
  });

  it("treats null/empty node defaults as unset", async () => {
    store.set("sepia:settings", JSON.stringify({ defaultAgent: null, defaultCwd: "" }));
    const loaded = await loadSettings();
    expect(loaded.defaultAgent).toEqual({});
    expect(loaded.defaultCwd).toEqual({});
  });

  it("accepts light and system themes", async () => {
    store.set("sepia:settings", JSON.stringify({ theme: "light" }));
    expect((await loadSettings()).theme).toBe("light");
    store.set("sepia:settings", JSON.stringify({ theme: "system" }));
    expect((await loadSettings()).theme).toBe("system");
  });

  it("normalizes stored sidebar sections against the defaults", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        sidebar: {
          sections: [
            { id: "folders", enabled: false },
            { id: "pinned", enabled: true, label: "Starred", limit: 3 },
            { id: "bogus", enabled: true },
            { id: "sessions", enabled: true, limit: -2 },
          ],
        },
      }),
    );
    const loaded = await loadSettings();
    // Stored order wins; unknown ids drop; missing ids append with defaults.
    expect(loaded.sidebar.sections.map((s) => s.id)).toEqual([
      "folders",
      "pinned",
      "sessions",
      "projects",
      "archived",
    ]);
    expect(loaded.sidebar.sections[0]).toMatchObject({ id: "folders", enabled: false });
    expect(loaded.sidebar.sections[1]).toMatchObject({ label: "Starred", limit: 3 });
    // Bad limit falls back to the section default.
    expect(loaded.sidebar.sections[2]).toMatchObject({ id: "sessions", limit: 8 });
  });

  it("restores stored keybind overrides verbatim", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({ keybinds: { "app.sidebar": "Mod+P", "nav.up": null } }),
    );
    expect((await loadSettings()).keybinds).toEqual({ "app.sidebar": "Mod+P", "nav.up": null });
  });

  it("drops corrupt keybind entries — only string|null survives", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        keybinds: { "app.sidebar": "Mod+P", "nav.up": 42, "nav.down": true, x: null },
      }),
    );
    expect((await loadSettings()).keybinds).toEqual({ "app.sidebar": "Mod+P", x: null });
  });

  it("localNodeEnabled defaults on — only an explicit false parks the local node", async () => {
    store.set("sepia:settings", JSON.stringify({ localNodeEnabled: false }));
    expect((await loadSettings()).localNodeEnabled).toBe(false);
    // Non-boolean legacy values read as enabled, same convention as peers.
    store.set("sepia:settings", JSON.stringify({ localNodeEnabled: "no" }));
    expect((await loadSettings()).localNodeEnabled).toBe(true);
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
    mod.setSettings({ theme: "light", defaultAgent: { local: "cline" } });
    expect(mod.settingsStore.state.theme).toBe("light");
    expect(mod.settingsStore.state.defaultAgent).toEqual({ local: "cline" });
    // Untouched keys keep their values.
    expect(mod.settingsStore.state.notifications).toEqual(DEFAULTS.notifications);

    const persisted = JSON.parse(store.get("sepia:settings") ?? "{}") as SepiaSettings;
    expect(persisted.theme).toBe("light");
    expect(persisted.defaultAgent).toEqual({ local: "cline" });
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

describe("node-scoped defaults", () => {
  const settings = (patch: Partial<SepiaSettings>): SepiaSettings => ({
    ...DEFAULTS,
    ...patch,
  });

  it("defaultAgentFor resolves the target node's entry — local forms share one slot", () => {
    const s = settings({ defaultAgent: { local: "devin", node_a1b2: "cline" } });
    expect(defaultAgentFor(s, undefined)).toBe("devin");
    expect(defaultAgentFor(s, "local")).toBe("devin");
    expect(defaultAgentFor(s, "node_a1b2")).toBe("cline");
    expect(defaultAgentFor(s, "node_zzz")).toBeNull();
  });

  it("defaultCwdFor resolves per node and misses to null", () => {
    const s = settings({ defaultCwd: { node_a1b2: "/peer/work" } });
    expect(defaultCwdFor(s, "node_a1b2")).toBe("/peer/work");
    expect(defaultCwdFor(s, undefined)).toBeNull();
    expect(defaultCwdFor(s, "local")).toBeNull();
  });

  it("withNodeDefault writes under the node key and clears on null/empty", () => {
    let map = withNodeDefault({}, "node_a1b2", "/peer/work");
    expect(map).toEqual({ node_a1b2: "/peer/work" });
    map = withNodeDefault(map, undefined, "/local/work");
    expect(map).toEqual({ node_a1b2: "/peer/work", local: "/local/work" });
    // Clearing one node leaves the other's entry alone.
    expect(withNodeDefault(map, "node_a1b2", null)).toEqual({ local: "/local/work" });
    expect(withNodeDefault(map, "local", "")).toEqual({ node_a1b2: "/peer/work" });
  });

  it("recentCwdFor picks the most recent session on the target node only", () => {
    const sessions = [
      { cwd: "/peer/newest", node: "node_a1b2" },
      { cwd: "/local/newest", node: "local" },
      { cwd: "/local/older" },
      { cwd: "/peer/older", node: "node_a1b2" },
    ];
    expect(recentCwdFor(sessions, undefined)).toBe("/local/newest");
    expect(recentCwdFor(sessions, "node_a1b2")).toBe("/peer/newest");
    expect(recentCwdFor(sessions, "node_zzz")).toBeNull();
    expect(recentCwdFor([], undefined)).toBeNull();
  });
});
