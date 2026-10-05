import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  desktopAgentFor,
  desktopCwdFor,
  desktopModelFor,
  recentCwdFor,
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

const EMPTY_DESKTOP = { node: null, agent: null, model: null, cwd: null };

const DEFAULTS: SepiaSettings = {
  desktop: EMPTY_DESKTOP,
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  localNodeUrl: null,
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
        desktop: "nope",
        models: "nope",
        keybinds: "no",
        notifications: "no",
        theme: "purple",
      }),
    );
    const loaded = await loadSettings();
    expect(loaded.desktop).toEqual(EMPTY_DESKTOP);
    expect(loaded.models).toEqual({});
    expect(loaded.keybinds).toEqual({});
    expect(loaded.notifications).toEqual({ enabled: false, done: true, permission: true });
    expect(loaded.theme).toBe("dark");
  });

  it("normalizes a stored desktop — non-string and empty fields drop to null", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        desktop: { node: "node_a1b2", agent: "", model: 7, cwd: "/work", extra: "x" },
      }),
    );
    expect((await loadSettings()).desktop).toEqual({
      node: "node_a1b2",
      agent: null,
      model: null,
      cwd: "/work",
    });
  });

  it("migrates the local node's legacy defaults into the desktop", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        defaultAgent: { local: "devin", node_a1b2: "cline", bad: 7, empty: "" },
        defaultCwd: { local: "/local/work", node_a1b2: "/peer/work" },
      }),
    );
    // Only the local entries migrate — peer entries were per-node defaults,
    // and the desktop is a single current environment.
    expect((await loadSettings()).desktop).toEqual({
      node: null,
      agent: "devin",
      model: null,
      cwd: "/local/work",
    });
  });

  it("migrates legacy scalar defaults (pre-federation) as the local entries", async () => {
    store.set("sepia:settings", JSON.stringify({ defaultAgent: "cline", defaultCwd: "/work" }));
    const loaded = await loadSettings();
    expect(loaded.desktop.agent).toBe("cline");
    expect(loaded.desktop.cwd).toBe("/work");
  });

  it("a stored desktop field wins over the legacy seed", async () => {
    store.set(
      "sepia:settings",
      JSON.stringify({
        desktop: { node: "node_a1b2", agent: "cursor" },
        defaultAgent: { local: "devin" },
        defaultCwd: "/legacy",
      }),
    );
    expect((await loadSettings()).desktop).toEqual({
      node: "node_a1b2",
      agent: "cursor",
      model: null,
      cwd: "/legacy",
    });
  });

  it("treats null/empty legacy defaults as unset", async () => {
    store.set("sepia:settings", JSON.stringify({ defaultAgent: null, defaultCwd: "" }));
    expect((await loadSettings()).desktop).toEqual(EMPTY_DESKTOP);
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

  it("localNodeUrl keeps only a canonical http(s) origin — the rest reads as no override", async () => {
    store.set("sepia:settings", JSON.stringify({ localNodeUrl: "http://thinkpad:8787" }));
    expect((await loadSettings()).localNodeUrl).toBe("http://thinkpad:8787");
    // A stored path is dropped — targets are origins, not endpoints.
    store.set("sepia:settings", JSON.stringify({ localNodeUrl: "http://thinkpad:8787/api" }));
    expect((await loadSettings()).localNodeUrl).toBe("http://thinkpad:8787");
    for (const bad of ["", "not a url", "ws://thinkpad:8787", 42]) {
      store.set("sepia:settings", JSON.stringify({ localNodeUrl: bad }));
      expect((await loadSettings()).localNodeUrl).toBeNull();
    }
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
    mod.setSettings({ theme: "light" });
    mod.setDesktop({ node: "node_a1b2", agent: "cline" });
    expect(mod.settingsStore.state.theme).toBe("light");
    expect(mod.settingsStore.state.desktop).toEqual({
      node: "node_a1b2",
      agent: "cline",
      model: null,
      cwd: null,
    });
    // Untouched keys keep their values; a later patch merges into the desktop.
    expect(mod.settingsStore.state.notifications).toEqual(DEFAULTS.notifications);
    mod.setDesktop({ model: "claude-x" });
    expect(mod.settingsStore.state.desktop.agent).toBe("cline");

    const persisted = JSON.parse(store.get("sepia:settings") ?? "{}") as SepiaSettings;
    expect(persisted.theme).toBe("light");
    expect(persisted.desktop).toEqual({
      node: "node_a1b2",
      agent: "cline",
      model: "claude-x",
      cwd: null,
    });
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

describe("desktop-scoped lookups", () => {
  const settings = (desktop: Partial<SepiaSettings["desktop"]>): SepiaSettings => ({
    ...DEFAULTS,
    desktop: { ...EMPTY_DESKTOP, ...desktop },
  });

  it("desktopAgentFor resolves only on the desktop's node — local forms share one slot", () => {
    const s = settings({ agent: "devin" });
    expect(desktopAgentFor(s, undefined)).toBe("devin");
    expect(desktopAgentFor(s, "local")).toBe("devin");
    expect(desktopAgentFor(s, "node_a1b2")).toBeNull();
    // A peer-scoped desktop doesn't leak its agent onto local creates.
    const peer = settings({ node: "node_a1b2", agent: "cline" });
    expect(desktopAgentFor(peer, "node_a1b2")).toBe("cline");
    expect(desktopAgentFor(peer, undefined)).toBeNull();
    expect(desktopAgentFor(peer, "node_zzz")).toBeNull();
  });

  it("desktopCwdFor resolves per node scope and misses to null", () => {
    const s = settings({ node: "node_a1b2", cwd: "/peer/work" });
    expect(desktopCwdFor(s, "node_a1b2")).toBe("/peer/work");
    expect(desktopCwdFor(s, undefined)).toBeNull();
    expect(desktopCwdFor(s, "local")).toBeNull();
  });

  it("desktopModelFor needs the desktop's node and (when picked) its agent", () => {
    const s = settings({ node: "node_a1b2", agent: "cline", model: "claude-x" });
    expect(desktopModelFor(s, "node_a1b2", "cline")).toBe("claude-x");
    // Wrong node or wrong agent → unset.
    expect(desktopModelFor(s, "node_zzz", "cline")).toBeNull();
    expect(desktopModelFor(s, "node_a1b2", "devin")).toBeNull();
    // No agent pick → the model applies to whatever agent runs there.
    const noAgent = settings({ model: "m1" });
    expect(desktopModelFor(noAgent, undefined, "devin")).toBe("m1");
    expect(desktopModelFor(noAgent, undefined, "cline")).toBe("m1");
    // No model pick → null regardless.
    expect(desktopModelFor(settings({ agent: "devin" }), undefined, "devin")).toBeNull();
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
