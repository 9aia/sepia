import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { focusNode, resolveCreateCwd, resolveCreateTarget } from "../lib/focus";
import { defaultSidebarSections } from "../lib/sidebar";
import type { SepiaSettings } from "../lib/settings";
import { sepiaStore, setFocus, type FocusTarget } from "../lib/store";

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
  setFocus(null);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const SETTINGS: SepiaSettings = {
  defaultAgent: {},
  defaultCwd: {},
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  sidebar: { sections: defaultSidebarSections() },
};

const settings = (patch: Partial<SepiaSettings>): SepiaSettings => ({ ...SETTINGS, ...patch });

describe("sepiaStore focus", () => {
  it("setFocus sets and clears the target", () => {
    expect(sepiaStore.state.focus).toBeNull();
    setFocus({ node: "node_a1b2", agent: "cline" });
    expect(sepiaStore.state.focus).toEqual({ node: "node_a1b2", agent: "cline" });
    setFocus(null);
    expect(sepiaStore.state.focus).toBeNull();
  });
});

describe("focusNode", () => {
  it("normalizes the local nodeKey to undefined for API call sites", () => {
    expect(focusNode(null)).toBeUndefined();
    expect(focusNode({ node: "local", agent: null })).toBeUndefined();
    expect(focusNode({ node: "node_a1b2", agent: null })).toBe("node_a1b2");
  });
});

describe("resolveCreateTarget", () => {
  it("no focus: the create stays local with the configured default agent", () => {
    const s = settings({ defaultAgent: { local: "devin" } });
    expect(resolveCreateTarget(null, s, undefined)).toEqual({ node: undefined, agent: "devin" });
    expect(resolveCreateTarget(null, settings({}), undefined)).toEqual({
      node: undefined,
      agent: null,
    });
  });

  it("a focused peer drives node + agent; null agent falls back to the node's default", () => {
    const s = settings({ defaultAgent: { node_a1b2: "cline", local: "devin" } });
    const focused: FocusTarget = { node: "node_a1b2", agent: "cursor" };
    expect(resolveCreateTarget(focused, s, undefined)).toEqual({
      node: "node_a1b2",
      agent: "cursor",
    });
    // agent null → the peer's own configured default, not the local one.
    expect(resolveCreateTarget({ node: "node_a1b2", agent: null }, s, undefined)).toEqual({
      node: "node_a1b2",
      agent: "cline",
    });
    // No peer default either → null (the peer picks).
    expect(resolveCreateTarget({ node: "node_zzz", agent: null }, s, undefined)).toEqual({
      node: "node_zzz",
      agent: null,
    });
  });

  it("a local focus resolves to undefined node but keeps its agent pick", () => {
    expect(resolveCreateTarget({ node: "local", agent: "cline" }, settings({}), undefined)).toEqual(
      { node: undefined, agent: "cline" },
    );
  });

  it("an explicit node always wins — 'New session here' rows ignore the focus", () => {
    const s = settings({ defaultAgent: { node_b: "cline" } });
    const focused: FocusTarget = { node: "node_a1b2", agent: "cursor" };
    expect(resolveCreateTarget(focused, s, "node_b")).toEqual({
      node: "node_b",
      agent: "cline",
    });
    // …and a focused agent doesn't leak onto a different node's create.
    expect(resolveCreateTarget(focused, s, "node_c")).toEqual({
      node: "node_c",
      agent: null,
    });
  });
});

describe("resolveCreateCwd", () => {
  const sessions = [
    { cwd: "/peer/newest", node: "node_a1b2" },
    { cwd: "/local/newest", node: "local" },
  ];

  it("local/no focus: picked > configured default > most recent local > homedir > /", () => {
    const s = settings({ defaultCwd: { local: "/local/default" } });
    const base = { settings: s, sessions, homedir: "/home/u" };
    // Picked wins over everything.
    expect(resolveCreateCwd(null, { ...base, picked: "/picked" })).toBe("/picked");
    // Configured default beats the recent-session dir and homedir.
    expect(resolveCreateCwd(null, { ...base, picked: null })).toBe("/local/default");
    // Then the newest local session's dir…
    expect(
      resolveCreateCwd(null, {
        settings: settings({}),
        sessions,
        picked: null,
        homedir: "/home/u",
      }),
    ).toBe("/local/newest");
    // …then home, then root.
    expect(
      resolveCreateCwd(null, { settings: settings({}), sessions: [], picked: null, homedir: "/h" }),
    ).toBe("/h");
    expect(
      resolveCreateCwd(null, {
        settings: settings({}),
        sessions: [],
        picked: null,
        homedir: undefined,
      }),
    ).toBe("/");
    // A local-node focus behaves identically.
    expect(resolveCreateCwd({ node: "local", agent: null }, { ...base, picked: "/picked" })).toBe(
      "/picked",
    );
  });

  it("a peer focus resolves dirs on that node — local picks and homedir don't leak", () => {
    const focus: FocusTarget = { node: "node_a1b2", agent: null };
    const s = settings({ defaultCwd: { node_a1b2: "/peer/default", local: "/local/default" } });
    expect(
      resolveCreateCwd(focus, { settings: s, sessions, picked: "/picked", homedir: "/home/u" }),
    ).toBe("/peer/default");
    expect(
      resolveCreateCwd(focus, {
        settings: settings({}),
        sessions,
        picked: "/picked",
        homedir: "/home/u",
      }),
    ).toBe("/peer/newest");
    expect(
      resolveCreateCwd(
        { node: "node_zzz", agent: null },
        {
          settings: settings({}),
          sessions,
          picked: "/picked",
          homedir: "/home/u",
        },
      ),
    ).toBe("/");
  });
});
