import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  clearFocus,
  focusNode,
  resolveCreateCwd,
  resolveCreateTarget,
  setFocus,
} from "../lib/focus";
import { defaultSidebarSections } from "../lib/sidebar";
import { settingsStore, type DesktopEnvironment, type SepiaSettings } from "../lib/settings";

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

const EMPTY_DESKTOP: DesktopEnvironment = { node: null, agent: null, model: null, cwd: null };

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
  setFocus(EMPTY_DESKTOP);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const SETTINGS: SepiaSettings = {
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

const settings = (desktop: Partial<DesktopEnvironment>): SepiaSettings => ({
  ...SETTINGS,
  desktop: { ...EMPTY_DESKTOP, ...desktop },
});

describe("the desktop is the persisted focus", () => {
  it("setFocus patches settings.desktop and persists it", () => {
    expect(settingsStore.state.desktop).toEqual(EMPTY_DESKTOP);
    setFocus({ node: "node_a1b2", agent: "cline" });
    expect(settingsStore.state.desktop).toEqual({
      ...EMPTY_DESKTOP,
      node: "node_a1b2",
      agent: "cline",
    });
    const persisted = JSON.parse(store.get("sepia:settings") ?? "{}") as SepiaSettings;
    expect(persisted.desktop).toEqual({ ...EMPTY_DESKTOP, node: "node_a1b2", agent: "cline" });
    clearFocus();
    expect(settingsStore.state.desktop).toEqual(EMPTY_DESKTOP);
  });

  it("setFocus merges — untouched fields keep their values", () => {
    setFocus({ node: "node_a1b2", cwd: "/peer/dir" });
    setFocus({ agent: "cline" });
    expect(settingsStore.state.desktop).toEqual({
      node: "node_a1b2",
      agent: "cline",
      model: null,
      cwd: "/peer/dir",
    });
  });
});

describe("focusNode", () => {
  it("normalizes the local nodeKey to undefined for API call sites", () => {
    expect(focusNode(EMPTY_DESKTOP)).toBeUndefined();
    expect(focusNode({ ...EMPTY_DESKTOP, node: "local" })).toBeUndefined();
    expect(focusNode({ ...EMPTY_DESKTOP, node: "node_a1b2" })).toBe("node_a1b2");
  });
});

describe("resolveCreateTarget", () => {
  it("an unset desktop: the create stays local and the node picks", () => {
    expect(resolveCreateTarget(settings({}), undefined)).toEqual({
      node: undefined,
      agent: null,
      model: null,
    });
  });

  it("a local desktop resolves to undefined node and keeps its picks", () => {
    const s = settings({ node: "local", agent: "cline", model: "claude-x" });
    expect(resolveCreateTarget(s, undefined)).toEqual({
      node: undefined,
      agent: "cline",
      model: "claude-x",
    });
  });

  it("a peer desktop drives node + agent + model", () => {
    const s = settings({ node: "node_a1b2", agent: "cursor", model: "gpt-y", cwd: "/peer/dir" });
    expect(resolveCreateTarget(s, undefined)).toEqual({
      node: "node_a1b2",
      agent: "cursor",
      model: "gpt-y",
    });
    // Unset agent/model → null (the peer picks).
    expect(resolveCreateTarget(settings({ node: "node_a1b2" }), undefined)).toEqual({
      node: "node_a1b2",
      agent: null,
      model: null,
    });
  });

  it("an explicit node always wins — 'New session here' rows ignore the desktop", () => {
    const s = settings({ node: "node_a1b2", agent: "cursor", model: "gpt-y" });
    expect(resolveCreateTarget(s, "node_b")).toEqual({
      node: "node_b",
      agent: null,
      model: null,
    });
    // …unless the row names the desktop's own node — then its picks apply.
    expect(resolveCreateTarget(s, "node_a1b2")).toEqual({
      node: "node_a1b2",
      agent: "cursor",
      model: "gpt-y",
    });
  });
});

describe("resolveCreateCwd", () => {
  const sessions = [
    { cwd: "/peer/newest", node: "node_a1b2" },
    { cwd: "/local/newest", node: "local" },
  ];

  it("local/unset desktop: picked > desktop cwd > most recent local > homedir > /", () => {
    const s = settings({ cwd: "/desktop/dir" });
    const base = { sessions, homedir: "/home/u" };
    // Picked wins over everything.
    expect(resolveCreateCwd(s, { ...base, picked: "/picked" })).toBe("/picked");
    // The desktop's dir pick beats the recent-session dir and homedir.
    expect(resolveCreateCwd(s, { ...base, picked: null })).toBe("/desktop/dir");
    // Then the newest local session's dir…
    expect(resolveCreateCwd(settings({}), { sessions, picked: null, homedir: "/home/u" })).toBe(
      "/local/newest",
    );
    // …then home, then root.
    expect(resolveCreateCwd(settings({}), { sessions: [], picked: null, homedir: "/h" })).toBe(
      "/h",
    );
    expect(resolveCreateCwd(settings({}), { sessions: [], picked: null, homedir: undefined })).toBe(
      "/",
    );
    // A local-node desktop behaves identically.
    expect(resolveCreateCwd(settings({ node: "local" }), { ...base, picked: "/picked" })).toBe(
      "/picked",
    );
  });

  it("a peer desktop resolves dirs on that node — local picks and homedir don't leak", () => {
    const s = settings({ node: "node_a1b2", cwd: "/peer/default" });
    expect(resolveCreateCwd(s, { sessions, picked: "/picked", homedir: "/home/u" })).toBe(
      "/peer/default",
    );
    expect(
      resolveCreateCwd(settings({ node: "node_a1b2" }), {
        sessions,
        picked: "/picked",
        homedir: "/home/u",
      }),
    ).toBe("/peer/newest");
    expect(
      resolveCreateCwd(settings({ node: "node_zzz" }), {
        sessions,
        picked: "/picked",
        homedir: "/home/u",
      }),
    ).toBe("/");
  });
});
