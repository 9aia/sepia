import { Store } from "@tanstack/react-store";
import { LOCAL_NODE_ID, nodeKey } from "./format";
import {
  defaultSidebarSections,
  normalizeSidebarSections,
  type SidebarSectionConfig,
} from "./sidebar";

/**
 * Client-side preferences, persisted to localStorage. Server-side
 * configuration (agents, spawn env) stays in `apps/server` env vars — these
 * are per-browser UI defaults only.
 */
/** Per-agent model preferences — spawn-time flags, not a live switch. */
export interface AgentModelPref {
  /** Preferred model (fuzzy ok for devin). Empty = agent default. */
  readonly model: string;
  /** Comma-separated fallback models tried when the primary refuses/fails. */
  readonly fallbacks: string;
  /** auto = pass fallbacks to the agent; manual = primary model only. */
  readonly mode: "auto" | "manual";
}

export interface SepiaSettings {
  /**
   * Agent id preselected when creating sessions, keyed by `nodeKey` —
   * "local" or a peer id. An absent key means the node's own default (the
   * create goes out with no agent override and the node picks).
   */
  defaultAgent: Record<string, string>;
  /**
   * cwd prefilled when creating sessions, keyed by `nodeKey`. An absent
   * key falls back to the most recent session's dir on that node.
   */
  defaultCwd: Record<string, string>;
  /** Per-agent model prefs keyed by agent id. */
  models: Record<string, AgentModelPref>;
  /** Keybind overrides by action id — string = custom key, null = disabled. */
  keybinds: Record<string, string | null>;
  /** Browser push toggles — enabled = subscribed on this device. */
  notifications: { enabled: boolean; done: boolean; permission: boolean };
  /** UI theme — dark default; "system" follows prefers-color-scheme. */
  theme: "dark" | "light" | "system";
  /** Nickname for this machine — overrides the self-reported node name. */
  localNodeName: string | null;
  /**
   * Settings → Nodes' enable switch for this machine — the local equivalent
   * of `PeerNode.enabled`. It's a client-local pref, not node state: `false`
   * stops this machine's sessions/projects/agents merging into the federated
   * lists (and closes its event feed), but its API stays reachable — the
   * origin is the transport every call lands on, not just a data source.
   */
  localNodeEnabled: boolean;
  /** Sidebar sections — array order is the render order. */
  sidebar: { sections: SidebarSectionConfig[] };
}

const KEY = "sepia:settings";

/**
 * Keep only `string` (custom key) and `null` (disabled) overrides — a corrupt
 * value like `{"session.new": 5}` would flow straight into useHotkey and throw.
 */
const sanitizeKeybinds = (value: unknown): Record<string, string | null> => {
  if (typeof value !== "object" || value === null) return {};
  const keybinds: Record<string, string | null> = {};
  for (const [id, override] of Object.entries(value)) {
    if (typeof override === "string" || override === null) keybinds[id] = override;
  }
  return keybinds;
};

/**
 * Node-scoped defaults load as `Record<nodeKey, value>`; a stored scalar
 * (pre-federation settings) migrates onto the local key so the pref
 * survives, and non-string entries drop.
 */
const normalizeNodeMap = (value: unknown): Record<string, string> => {
  if (typeof value === "string") return value === "" ? {} : { [LOCAL_NODE_ID]: value };
  if (typeof value !== "object" || value === null) return {};
  const map: Record<string, string> = {};
  for (const [node, entry] of Object.entries(value)) {
    if (typeof entry === "string" && entry !== "") map[node] = entry;
  }
  return map;
};

const defaultSettings = (): SepiaSettings => ({
  defaultAgent: {},
  defaultCwd: {},
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  localNodeEnabled: true,
  sidebar: { sections: defaultSidebarSections() },
});

const load = (): SepiaSettings => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return defaultSettings();
    const parsed = JSON.parse(raw) as Partial<SepiaSettings>;
    return {
      defaultAgent: normalizeNodeMap(parsed.defaultAgent),
      defaultCwd: normalizeNodeMap(parsed.defaultCwd),
      models:
        typeof parsed.models === "object" && parsed.models !== null
          ? (parsed.models as Record<string, AgentModelPref>)
          : {},
      keybinds: sanitizeKeybinds(parsed.keybinds),
      notifications:
        typeof parsed.notifications === "object" && parsed.notifications !== null
          ? {
              enabled: parsed.notifications.enabled === true,
              done: parsed.notifications.done !== false,
              permission: parsed.notifications.permission !== false,
            }
          : { enabled: false, done: true, permission: true },
      theme: parsed.theme === "light" || parsed.theme === "system" ? parsed.theme : "dark",
      localNodeName:
        typeof parsed.localNodeName === "string" && parsed.localNodeName.trim() !== ""
          ? parsed.localNodeName.trim()
          : null,
      // Absent (and any non-false legacy value) reads as enabled — same
      // convention as `PeerNode.enabled`.
      localNodeEnabled: parsed.localNodeEnabled !== false,
      sidebar: {
        sections: normalizeSidebarSections(
          typeof parsed.sidebar === "object" && parsed.sidebar !== null
            ? (parsed.sidebar as { sections?: unknown }).sections
            : undefined,
        ),
      },
    };
  } catch {
    return defaultSettings();
  }
};

const persist = (settings: SepiaSettings): void => {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // Storage unavailable (private mode); settings live for the session.
  }
};

export const settingsStore = new Store<SepiaSettings>(load());

export const setSettings = (patch: Partial<SepiaSettings>): void => {
  settingsStore.setState((prev) => {
    const next = { ...prev, ...patch };
    persist(next);
    return next;
  });
};

// --- Node-scoped creation defaults --------------------------------------------
// `node` is the creation target's node field — undefined/"local"/the issued
// local id all normalize through `nodeKey`, so a peer id and the local row
// forms resolve to the same slot.

/** The configured default agent for `node` — null = the node picks. */
export const defaultAgentFor = (settings: SepiaSettings, node: string | undefined): string | null =>
  settings.defaultAgent[nodeKey(node)] ?? null;

/** The configured spawn dir for `node` — null = fall back to recents/home. */
export const defaultCwdFor = (settings: SepiaSettings, node: string | undefined): string | null =>
  settings.defaultCwd[nodeKey(node)] ?? null;

/**
 * Write one node's entry in a node-scoped defaults map. `null`/`""` removes
 * the key — an unset slot falls back rather than storing an explicit empty.
 */
export const withNodeDefault = (
  map: Record<string, string>,
  node: string | undefined,
  value: string | null,
): Record<string, string> => {
  const key = nodeKey(node);
  const next = { ...map };
  if (value === null || value === "") delete next[key];
  else next[key] = value;
  return next;
};

/**
 * The most recent session's cwd on `node`. The input list is newest-first
 * (`useSessions` orders by updatedAt desc), so the first node match is the
 * latest — a peer's paths are never a valid local spawn fallback.
 */
export const recentCwdFor = (
  sessions: ReadonlyArray<{ readonly cwd: string; readonly node?: string }>,
  node: string | undefined,
): string | null => {
  const key = nodeKey(node);
  return sessions.find((session) => nodeKey(session.node) === key)?.cwd ?? null;
};
