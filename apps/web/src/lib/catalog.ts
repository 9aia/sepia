import { AUTO_MODEL_ID, agentKey, catalogKey, nodeKey } from "./format";
import { setSettings, settingsStore, type AgentModelPref, type SepiaSettings } from "./settings";

/**
 * The federated agent/model catalog — what Settings → Agents/Models/Nodes
 * and the pick menus consume.
 *
 * Gathering (`useCatalog`): for each enabled node (this machine plus every
 * enabled peer) the node's agent roster — `self.agents` locally, the peer's
 * `/api/node` descriptor, the merged `useAgents` roster as the pre-probe
 * fallback — and, per agent, its model set. Agents don't advertise models
 * on the wire, so an agent's models are the client's own configured prefs
 * (`settings.models[agentId]` — primary + comma-separated fallbacks) plus
 * the `auto` agent-default entry; if agents ever grow a model
 * advertisement, this builder is where it merges.
 *
 * Every entity is per-occurrence and carries its own toggle key: an agent
 * row is `node:agent`, a model row `node:agent:model` (lib/format.ts). The
 * `agents`/`nodes`/`models` cross-relations record which other agents and
 * nodes carry the same id, so a row can render "also on thinkpad".
 */

/** A model id as offered by one agent on one node. */
export interface CatalogModel {
  /** The model id — `AUTO_MODEL_ID` ("auto") marks the agent-default entry. */
  readonly id: string;
  /** `node:agent:model` — the `disabledModels` toggle key. */
  readonly key: string;
  /** Menu label — the id, or "Agent default" for the auto entry. */
  readonly label: string;
  /** Every agent id carrying this model id somewhere in the catalog. */
  readonly agents: ReadonlyArray<string>;
  /** Every node (nodeKey) where this model id is offered. */
  readonly nodes: ReadonlyArray<string>;
  /** The model's own toggle — false when `disabledModels` lists `key`. */
  readonly enabled: boolean;
  /** True for the agent-default sentinel entry — not a real model id. */
  readonly auto: boolean;
}

/** An agent as offered on one node. */
export interface CatalogAgent {
  /** The agent id, e.g. "devin". */
  readonly id: string;
  /** Display label — the roster's AgentInfo label, else the id. */
  readonly label: string;
  /** `node:agent` — the `disabledAgents` toggle key. */
  readonly key: string;
  /** The node (nodeKey) this row is offered on. */
  readonly node: string;
  /** Every catalog node where this agent id appears. */
  readonly nodes: ReadonlyArray<string>;
  /** The model set — the auto entry first, then the configured pref ids. */
  readonly models: ReadonlyArray<CatalogModel>;
  /** The agent's own toggle — false when `disabledAgents` lists `key`. */
  readonly enabled: boolean;
}

/** One enabled node and its agent roster. */
export interface CatalogNode {
  /** nodeKey — "local" for this machine, the peer id otherwise. */
  readonly node: string;
  /** Display label — nicknames win, then reported names. */
  readonly label: string;
  readonly agents: ReadonlyArray<CatalogAgent>;
}

export interface Catalog {
  /** Enabled nodes in display order — this machine first, then peers. */
  readonly nodes: ReadonlyArray<CatalogNode>;
  /**
   * Flat per-(node,agent) rows — the same objects `nodes[].agents` holds,
   * so Settings → Agents can render a flat list or group by node/id.
   */
  readonly agents: ReadonlyArray<CatalogAgent>;
  /**
   * Flat per-(node,agent,model) rows — Settings → Models' row set; group
   * by `id` for a federated "who carries this model" view.
   */
  readonly models: ReadonlyArray<CatalogModel>;
}

// --- Disabled-flag predicates --------------------------------------------------
// The flags take any settings-shaped object so callers holding a useStore
// slice ({disabledAgents}/{disabledModels}) don't need the whole record.

/** Whether the agent is offered on `node` — false when parked in `disabledAgents`. */
export const isAgentEnabled = (
  settings: Pick<SepiaSettings, "disabledAgents">,
  node: string | undefined,
  agentId: string,
): boolean => !settings.disabledAgents.includes(agentKey(node, agentId));

/**
 * Whether the model is offered for `agentId` on `node`. A null/empty model
 * id checks the agent-default (`auto`) entry — parking `node:agent:auto`
 * hides the "Agent default" affordance in pickers but can't block spawns:
 * "no model arg" IS the agent default, so an explicit model always slips
 * past. A parked agent parks its whole subtree — check `isAgentEnabled`
 * too where both matter.
 */
export const isModelEnabled = (
  settings: Pick<SepiaSettings, "disabledModels">,
  node: string | undefined,
  agentId: string,
  modelId: string | null | undefined,
): boolean => !settings.disabledModels.includes(catalogKey(node, agentId, modelId));

// --- Toggles -------------------------------------------------------------------
// Synchronous settings writes — the Settings sections wire these to switches;
// parked items stay in the list so re-enabling is a key removal, not a refetch.

const toggleKey = (list: ReadonlyArray<string>, key: string, enabled: boolean): string[] =>
  enabled ? list.filter((entry) => entry !== key) : list.includes(key) ? [...list] : [...list, key];

/** Park/unpark an agent on a node (`settings.disabledAgents`). */
export const setAgentEnabled = (
  node: string | undefined,
  agentId: string,
  enabled: boolean,
): void => {
  setSettings({
    disabledAgents: toggleKey(settingsStore.state.disabledAgents, agentKey(node, agentId), enabled),
  });
};

/** Park/unpark a model on a node+agent (`settings.disabledModels`); `null` model = the auto entry. */
export const setModelEnabled = (
  node: string | undefined,
  agentId: string,
  modelId: string | null | undefined,
  enabled: boolean,
): void => {
  setSettings({
    disabledModels: toggleKey(
      settingsStore.state.disabledModels,
      catalogKey(node, agentId, modelId),
      enabled,
    ),
  });
};

// --- Model sets ------------------------------------------------------------------

/** The named models a configured pref lists — primary + fallbacks, trimmed, deduped. */
export const prefModelIds = (pref: AgentModelPref | undefined): string[] => {
  if (pref === undefined) return [];
  return [
    ...new Set(
      [pref.model, ...pref.fallbacks.split(",")].map((id) => id.trim()).filter((id) => id !== ""),
    ),
  ];
};

/**
 * An agent's catalog model ids — the `auto` agent-default entry first, then
 * the configured pref's primary + fallback ids (duplicates fold). This is
 * the whole model set: agents don't advertise models on the wire, so the
 * client's configured prefs plus the default are all there is.
 */
export const agentModelIds = (
  settings: Pick<SepiaSettings, "models">,
  agentId: string,
): string[] => [AUTO_MODEL_ID, ...prefModelIds(settings.models[agentId])];

// --- Gathering -------------------------------------------------------------------

/** One node's roster as buildCatalog input — ids plus display labels. */
export interface CatalogNodeInput {
  /** nodeKey — "local" or the peer id (normalized inside). */
  readonly node: string;
  readonly label: string;
  readonly agents: ReadonlyArray<{ readonly id: string; readonly label: string }>;
}

const pushUnique = (map: Map<string, string[]>, key: string, value: string): void => {
  const list = map.get(key);
  if (list === undefined) map.set(key, [value]);
  else if (!list.includes(value)) list.push(value);
};

/**
 * Build the catalog from gathered node rosters — pure, so tests drive it
 * without stores. `input` lists enabled nodes in display order. Duplicate
 * agent ids on one node fold to a single row; cross-relations collect in
 * first-seen order.
 */
export const buildCatalog = (
  input: ReadonlyArray<CatalogNodeInput>,
  settings: Pick<SepiaSettings, "models" | "disabledAgents" | "disabledModels">,
): Catalog => {
  // Pass 1 — cross-relations: which nodes carry each agent id, which
  // agents+nodes carry each model id.
  const agentNodes = new Map<string, string[]>();
  const modelAgents = new Map<string, string[]>();
  const modelNodes = new Map<string, string[]>();
  for (const entry of input) {
    const node = nodeKey(entry.node);
    const seen = new Set<string>();
    for (const agent of entry.agents) {
      if (seen.has(agent.id)) continue;
      seen.add(agent.id);
      pushUnique(agentNodes, agent.id, node);
      for (const modelId of agentModelIds(settings, agent.id)) {
        pushUnique(modelAgents, modelId, agent.id);
        pushUnique(modelNodes, modelId, node);
      }
    }
  }

  // Pass 2 — emit rows. `enabled` is each row's own toggle; a parked agent's
  // models keep their own flag (the agent row carries the parked state).
  const disabledAgents = new Set(settings.disabledAgents);
  const disabledModels = new Set(settings.disabledModels);
  const nodes: CatalogNode[] = [];
  const agents: CatalogAgent[] = [];
  const models: CatalogModel[] = [];
  for (const entry of input) {
    const node = nodeKey(entry.node);
    const seen = new Set<string>();
    const nodeAgents: CatalogAgent[] = [];
    for (const agent of entry.agents) {
      if (seen.has(agent.id)) continue;
      seen.add(agent.id);
      const aKey = agentKey(node, agent.id);
      const agentModels = agentModelIds(settings, agent.id).map((id) => {
        const auto = id === AUTO_MODEL_ID;
        const row: CatalogModel = {
          id,
          key: catalogKey(node, agent.id, id),
          label: auto ? "Agent default" : id,
          agents: modelAgents.get(id) ?? [],
          nodes: modelNodes.get(id) ?? [],
          enabled: !disabledModels.has(catalogKey(node, agent.id, id)),
          auto,
        };
        models.push(row);
        return row;
      });
      const row: CatalogAgent = {
        id: agent.id,
        label: agent.label,
        key: aKey,
        node,
        nodes: agentNodes.get(agent.id) ?? [node],
        models: agentModels,
        enabled: !disabledAgents.has(aKey),
      };
      nodeAgents.push(row);
      agents.push(row);
    }
    nodes.push({ node, label: entry.label, agents: nodeAgents });
  }
  return { nodes, agents, models };
};

// --- Consumption helpers ---------------------------------------------------------

/** The roster's first agent enabled on `node` — undefined when all are parked. */
export const firstEnabledAgentId = (
  settings: Pick<SepiaSettings, "disabledAgents">,
  node: string | undefined,
  rosterIds: ReadonlyArray<string>,
): string | undefined => rosterIds.find((id) => isAgentEnabled(settings, node, id));

/**
 * The agent a create should use: `picked` when it's enabled on `node`,
 * else the roster's first enabled id (pass `[]` for "the node picks" —
 * peer creates ship no override since a local-only id would fail there).
 */
export const enabledAgentOr = (
  settings: Pick<SepiaSettings, "disabledAgents">,
  node: string | undefined,
  picked: string | null | undefined,
  rosterIds: ReadonlyArray<string>,
): string | undefined =>
  picked !== null && picked !== undefined && isAgentEnabled(settings, node, picked)
    ? picked
    : rosterIds.find((id) => isAgentEnabled(settings, node, id));
