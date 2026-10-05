import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  agentKey,
  AUTO_MODEL_ID,
  catalogKey,
  parseAgentKey,
  parseCatalogKey,
  sameCatalogKey,
  setLocalNodeAlias,
} from "../lib/format";
import {
  agentModelIds,
  buildCatalog,
  enabledAgentOr,
  firstEnabledAgentId,
  isAgentEnabled,
  isModelEnabled,
  prefModelIds,
  setAgentEnabled,
  setModelEnabled,
  type CatalogNodeInput,
} from "../lib/catalog";
import { modelArgsFor } from "../lib/models";
import { resolveCreateTarget } from "../lib/focus";
import { settingsStore, type SepiaSettings } from "../lib/settings";
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

const settings = (overrides: Partial<SepiaSettings> = {}): SepiaSettings => ({
  desktop: { node: null, agent: null, model: null, cwd: null },
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  localNodeUrl: null,
  localNodeEnabled: true,
  disabledAgents: [],
  disabledModels: [],
  sidebar: { sections: defaultSidebarSections() },
  ...overrides,
});

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
  settingsStore.setState((prev) => ({ ...prev, disabledAgents: [], disabledModels: [] }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("catalog keys", () => {
  it("mint node:agent and node:agent:model forms on nodeKey segments", () => {
    expect(agentKey(undefined, "devin")).toBe("local:devin");
    expect(agentKey("local", "cline")).toBe("local:cline");
    expect(agentKey("node_x", "cline")).toBe("node_x:cline");
    expect(catalogKey("local", "devin", "claude-opus-4")).toBe("local:devin:claude-opus-4");
    expect(catalogKey("node_x", "cline", "gpt-y")).toBe("node_x:cline:gpt-y");
  });

  it("a missing model id mints the auto sentinel entry", () => {
    expect(catalogKey("local", "devin", null)).toBe("local:devin:auto");
    expect(catalogKey("local", "devin", undefined)).toBe("local:devin:auto");
    expect(catalogKey("local", "devin", "")).toBe("local:devin:auto");
    expect(catalogKey("local", "devin", AUTO_MODEL_ID)).toBe("local:devin:auto");
  });

  it("parses keys back — the model segment keeps embedded colons", () => {
    expect(parseAgentKey("local:devin")).toEqual({ node: "local", agent: "devin" });
    expect(parseAgentKey("local:devin:m1")).toBeNull();
    expect(parseAgentKey("devin")).toBeNull();
    expect(parseAgentKey("local:")).toBeNull();
    expect(parseCatalogKey("node_x:cline:openrouter:gpt-5")).toEqual({
      node: "node_x",
      agent: "cline",
      model: "openrouter:gpt-5",
    });
    expect(parseCatalogKey("local:devin")).toBeNull();
  });

  it("compares semantically — the local alias resolves to the sentinel", () => {
    setLocalNodeAlias("node_self1");
    expect(agentKey("node_self1", "devin")).toBe("local:devin");
    expect(sameCatalogKey("node_self1:devin:m1", "local:devin:m1")).toBe(true);
    expect(sameCatalogKey("local:devin:m1", "local:cline:m1")).toBe(false);
    expect(sameCatalogKey("local:devin:m1", "node_x:devin:m1")).toBe(false);
  });
});

describe("prefModelIds / agentModelIds", () => {
  it("collects primary + fallbacks, trimmed and deduped", () => {
    expect(prefModelIds({ model: " m1 ", fallbacks: "m2, m1 ,, ", mode: "auto" })).toEqual([
      "m1",
      "m2",
    ]);
    expect(prefModelIds(undefined)).toEqual([]);
  });

  it("agentModelIds leads with the auto default entry", () => {
    const s = settings({ models: { devin: { model: "m1", fallbacks: "m2", mode: "auto" } } });
    expect(agentModelIds(s, "devin")).toEqual(["auto", "m1", "m2"]);
    expect(agentModelIds(s, "cline")).toEqual(["auto"]);
  });
});

const NODES: CatalogNodeInput[] = [
  {
    node: "local",
    label: "this machine",
    agents: [
      { id: "devin", label: "Devin" },
      { id: "cline", label: "Cline" },
    ],
  },
  {
    node: "node_x",
    label: "thinkpad",
    agents: [
      { id: "cline", label: "Cline" },
      { id: "cursor", label: "Cursor" },
    ],
  },
];

describe("buildCatalog", () => {
  const prefs = settings({
    models: {
      devin: { model: "claude-opus-4", fallbacks: "claude-sonnet-4", mode: "auto" },
      cline: { model: "gpt-y", fallbacks: "", mode: "manual" },
    },
  });

  it("builds per-node agent rosters with the auto + configured models", () => {
    const catalog = buildCatalog(NODES, prefs);
    expect(catalog.nodes.map((n) => n.node)).toEqual(["local", "node_x"]);
    const local = catalog.nodes[0];
    expect(local?.agents.map((a) => a.id)).toEqual(["devin", "cline"]);
    const devin = local?.agents[0];
    expect(devin?.models.map((m) => m.id)).toEqual(["auto", "claude-opus-4", "claude-sonnet-4"]);
    expect(devin?.models[0]?.key).toBe("local:devin:auto");
    expect(devin?.models[1]?.key).toBe("local:devin:claude-opus-4");
    expect(devin?.models[0]?.auto).toBe(true);
    expect(devin?.models[1]?.auto).toBe(false);
  });

  it("computes the cross-relations — agents' nodes, models' carriers", () => {
    const catalog = buildCatalog(NODES, prefs);
    // cline is on both nodes; devin local-only.
    const cline = catalog.agents.find((a) => a.key === "local:cline");
    expect(cline?.nodes).toEqual(["local", "node_x"]);
    expect(catalog.agents.find((a) => a.key === "node_x:devin")).toBeUndefined();
    // gpt-y is configured for cline → offered wherever cline appears.
    const gptY = catalog.models.find((m) => m.key === "node_x:cline:gpt-y");
    expect(gptY?.agents).toEqual(["cline"]);
    expect(gptY?.nodes).toEqual(["local", "node_x"]);
    // auto is offered by every agent on every node.
    const auto = catalog.models.find((m) => m.key === "local:devin:auto");
    expect(auto?.agents).toEqual(["devin", "cline", "cursor"]);
    expect(auto?.nodes).toEqual(["local", "node_x"]);
    // opus is devin-only → single carrier.
    const opus = catalog.models.find((m) => m.key === "local:devin:claude-opus-4");
    expect(opus?.agents).toEqual(["devin"]);
    expect(opus?.nodes).toEqual(["local"]);
  });

  it("flattens agents and models for the sections' row lists", () => {
    const catalog = buildCatalog(NODES, prefs);
    expect(catalog.agents.map((a) => a.key)).toEqual([
      "local:devin",
      "local:cline",
      "node_x:cline",
      "node_x:cursor",
    ]);
    // Flat models: auto+gpt-y per cline row, cursor gets just auto.
    expect(catalog.models.map((m) => m.key)).toContain("node_x:cursor:auto");
    expect(catalog.models.filter((m) => m.id === "gpt-y")).toHaveLength(2);
  });

  it("marks parked agents and models, keeping the rows", () => {
    const catalog = buildCatalog(NODES, {
      ...prefs,
      disabledAgents: ["local:cline"],
      disabledModels: ["local:devin:claude-opus-4", "node_x:cursor:auto"],
    });
    expect(catalog.agents.find((a) => a.key === "local:cline")?.enabled).toBe(false);
    expect(catalog.agents.find((a) => a.key === "node_x:cline")?.enabled).toBe(true);
    expect(catalog.models.find((m) => m.key === "local:devin:claude-opus-4")?.enabled).toBe(false);
    expect(catalog.models.find((m) => m.key === "node_x:cursor:auto")?.enabled).toBe(false);
    // A parked model on one node doesn't park the same id elsewhere.
    expect(catalog.models.find((m) => m.key === "local:devin:claude-sonnet-4")?.enabled).toBe(true);
  });

  it("folds duplicate agent ids on one node", () => {
    const catalog = buildCatalog(
      [
        {
          node: "local",
          label: "l",
          agents: [
            { id: "devin", label: "D" },
            { id: "devin", label: "D" },
          ],
        },
      ],
      settings({}),
    );
    expect(catalog.agents).toHaveLength(1);
  });
});

describe("enabled predicates", () => {
  it("check the scoped key — parked on one node+agent only", () => {
    const s = settings({
      disabledAgents: ["local:devin"],
      disabledModels: ["local:devin:m1", "node_x:cline:auto"],
    });
    expect(isAgentEnabled(s, "local", "devin")).toBe(false);
    expect(isAgentEnabled(s, "local", "cline")).toBe(true);
    expect(isAgentEnabled(s, "node_x", "devin")).toBe(true);
    expect(isModelEnabled(s, "local", "devin", "m1")).toBe(false);
    expect(isModelEnabled(s, "local", "devin", "m2")).toBe(true);
    expect(isModelEnabled(s, "node_x", "devin", "m1")).toBe(true);
    // null checks the agent-default (auto) entry.
    expect(isModelEnabled(s, "node_x", "cline", null)).toBe(false);
    expect(isModelEnabled(s, "local", "cline", null)).toBe(true);
  });
});

describe("toggles", () => {
  it("setAgentEnabled parks and unparks via disabledAgents", () => {
    setAgentEnabled("local", "devin", false);
    expect(settingsStore.state.disabledAgents).toEqual(["local:devin"]);
    // Toggling off twice doesn't double-list.
    setAgentEnabled(undefined, "devin", false);
    expect(settingsStore.state.disabledAgents).toEqual(["local:devin"]);
    setAgentEnabled("local", "devin", true);
    expect(settingsStore.state.disabledAgents).toEqual([]);
    const persisted = JSON.parse(store.get("sepia:settings") ?? "{}") as SepiaSettings;
    expect(persisted.disabledAgents).toEqual([]);
  });

  it("setModelEnabled keys the auto entry when the model is null", () => {
    setModelEnabled("node_x", "cline", "gpt-y", false);
    setModelEnabled("node_x", "cline", null, false);
    expect(settingsStore.state.disabledModels).toEqual(["node_x:cline:gpt-y", "node_x:cline:auto"]);
    setModelEnabled("node_x", "cline", "gpt-y", true);
    expect(settingsStore.state.disabledModels).toEqual(["node_x:cline:auto"]);
  });
});

describe("consumption gating", () => {
  it("resolveCreateTarget drops a parked agent/model pick", () => {
    const s = settings({
      desktop: { node: "local", agent: "devin", model: "m1", cwd: null },
      disabledAgents: ["local:devin"],
    });
    expect(resolveCreateTarget(s, undefined)).toEqual({
      node: undefined,
      agent: null,
      model: "m1",
    });
    // Model parked under the desktop's agent → unset too.
    const s2 = settings({
      desktop: { node: "local", agent: "devin", model: "m1", cwd: null },
      disabledModels: ["local:devin:m1"],
    });
    expect(resolveCreateTarget(s2, undefined).model).toBeNull();
    // A pick parked on another node still applies locally.
    const s3 = settings({
      desktop: { node: null, agent: "devin", model: "m1", cwd: null },
      disabledModels: ["node_x:devin:m1"],
      disabledAgents: ["node_x:devin"],
    });
    expect(resolveCreateTarget(s3, undefined)).toEqual({
      node: undefined,
      agent: "devin",
      model: "m1",
    });
  });

  it("enabledAgentOr falls to the first enabled roster agent", () => {
    const s = settings({ disabledAgents: ["local:devin"] });
    expect(enabledAgentOr(s, "local", "devin", ["devin", "cline"])).toBe("cline");
    expect(enabledAgentOr(s, "local", "cline", ["devin", "cline"])).toBe("cline");
    expect(enabledAgentOr(s, "local", null, ["devin", "cline"])).toBe("cline");
    expect(enabledAgentOr(s, "local", null, [])).toBeUndefined();
    // All parked → nothing.
    const all = settings({ disabledAgents: ["local:devin", "local:cline"] });
    expect(enabledAgentOr(all, "local", null, ["devin", "cline"])).toBeUndefined();
    expect(firstEnabledAgentId(all, "local", ["devin", "cline"])).toBeUndefined();
  });

  it("modelArgsFor skips parked models and falls through to the next source", () => {
    const s = settings({
      models: { devin: { model: "configured", fallbacks: "m2,m3", mode: "auto" } },
    });
    const desktop = {
      ...s,
      desktop: { node: null, agent: "devin", model: "desktop-m", cwd: null },
    };
    // Session model parked → the desktop pick applies instead.
    const parked = { ...desktop, disabledModels: ["local:devin:session-m"] };
    expect(modelArgsFor("devin", "session-m", parked).model).toBe("desktop-m");
    // Desktop + configured both parked → nothing is emitted.
    const allParked = {
      ...desktop,
      disabledModels: ["local:devin:session-m", "local:devin:desktop-m", "local:devin:configured"],
    };
    expect(modelArgsFor("devin", "session-m", allParked).model).toBeUndefined();
    // Parked fallbacks drop individually.
    const fb = { ...s, disabledModels: ["local:devin:m2"] };
    expect(modelArgsFor("devin", null, fb).fallbacks).toEqual(["m3"]);
  });

  it("modelArgsFor scopes the check to the resolved node+agent", () => {
    const s = settings({
      models: { devin: { model: "m1", fallbacks: "", mode: "manual" } },
      disabledModels: ["node_x:devin:m1"],
    });
    expect(modelArgsFor("devin", null, s, "node_x").model).toBeUndefined();
    expect(modelArgsFor("devin", null, s, "local").model).toBe("m1");
    // The same model id under a different agent is unaffected.
    expect(modelArgsFor("cline", "m1", s, "node_x").model).toBe("m1");
  });
});
