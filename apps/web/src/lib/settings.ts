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
 * are per-client UI defaults only.
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

/**
 * The client's current working environment — which machine, agent, model,
 * and directory "New session" aims at. This is the persisted *focus*: the
 * sidebar footer's picks and the Settings → Desktop section both write
 * here. Every field is optional; unset fields fall back —
 * `node` → this machine, `agent` → the node's own pick, `model` → the
 * agent's configured pref (`settings.models`), `cwd` → the most recent
 * session's directory on that node. `agent`/`model`/`cwd` only apply on
 * `node` — they're scoped picks, not global defaults.
 */
export interface DesktopEnvironment {
  /** nodeKey — "local" or a peer id; `null` = this machine. */
  readonly node: string | null;
  /** Agent id on `node`; `null` = the node picks. */
  readonly agent: string | null;
  /** Spawn-time model for the desktop's agent; `null` = agent pref/default. */
  readonly model: string | null;
  /** Working directory on `node`; `null` = most recent session's dir. */
  readonly cwd: string | null;
}

export interface SepiaSettings {
  /** The current desktop — the machine+agent+model+dir creates target. */
  desktop: DesktopEnvironment;
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
   * Address override for the local node — the machine this client treats as
   * "local". `null` (default) means the origin serving this UI: local API
   * calls go out relative (`baseUrl: ""`), which is also what makes the vite
   * dev proxy (UI :3000 → API :8787) work. Set it to a canonical http(s)
   * origin to point the client at a different node than the UI host — every
   * local call (fan-out legs, session actions, the events feed, gateway
   * hops) then goes to that absolute origin with the local token.
   */
  localNodeUrl: string | null;
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

/** A non-empty string field reads through; anything else is unset. */
const stringOrNull = (value: unknown): string | null =>
  typeof value === "string" && value !== "" ? value : null;

/**
 * The legacy node-scoped default maps (`Record<nodeKey, value>`, or a
 * pre-federation scalar read as the local entry) survive only as a desktop
 * seed — the local node's entries become the current environment's agent/
 * cwd, and every peer entry drops. Returns `null` when nothing stored.
 */
const legacyLocalDefault = (value: unknown): string | null => {
  if (typeof value === "string") return stringOrNull(value);
  if (typeof value !== "object" || value === null) return null;
  const entry = (value as Record<string, unknown>)[LOCAL_NODE_ID];
  return stringOrNull(entry);
};

/**
 * `settings.desktop` — each field keeps only a non-empty string. The legacy
 * per-node `defaultAgent`/`defaultCwd` maps seed unset fields from their
 * local entry, so a pre-desktop store keeps its local picks; a `desktop`
 * field that's already set always wins over the legacy seed.
 */
const normalizeDesktop = (
  value: unknown,
  legacyAgent: unknown,
  legacyCwd: unknown,
): DesktopEnvironment => {
  const raw: Record<string, unknown> =
    typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  return {
    node: stringOrNull(raw.node),
    agent: stringOrNull(raw.agent) ?? legacyLocalDefault(legacyAgent),
    model: stringOrNull(raw.model),
    cwd: stringOrNull(raw.cwd) ?? legacyLocalDefault(legacyCwd),
  };
};

/**
 * The stored local-node address override — kept only as a canonical http(s)
 * origin; anything else (empty, malformed, non-http scheme, a stray path —
 * origins are all `localTarget` can use) reads as "no override".
 */
const normalizeLocalNodeUrl = (value: unknown): string | null => {
  if (typeof value !== "string" || value.trim() === "") return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === "http:" || url.protocol === "https:" ? url.origin : null;
  } catch {
    return null;
  }
};

const defaultSettings = (): SepiaSettings => ({
  desktop: { node: null, agent: null, model: null, cwd: null },
  models: {},
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  localNodeName: null,
  localNodeUrl: null,
  localNodeEnabled: true,
  sidebar: { sections: defaultSidebarSections() },
});

const load = (): SepiaSettings => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return defaultSettings();
    const parsed = JSON.parse(raw) as Partial<SepiaSettings>;
    // Pre-desktop stores carried per-node `defaultAgent`/`defaultCwd` maps;
    // their local entries seed the desktop (see normalizeDesktop).
    const legacy = parsed as Record<string, unknown>;
    return {
      desktop: normalizeDesktop(parsed.desktop, legacy.defaultAgent, legacy.defaultCwd),
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
      localNodeUrl: normalizeLocalNodeUrl(parsed.localNodeUrl),
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

/** Merge a partial desktop pick into `settings.desktop` and persist. */
export const setDesktop = (patch: Partial<DesktopEnvironment>): void =>
  setSettings({ desktop: { ...settingsStore.state.desktop, ...patch } });

// --- Desktop-scoped lookups ---------------------------------------------------
// `node` is the caller's node field — undefined/"local"/the issued local id
// all normalize through `nodeKey`, and the desktop's `agent`/`cwd`/`model`
// only resolve when `node` is the desktop's own machine: a pick for a peer
// is never a valid local value (or vice versa).

/** Whether `node` is the machine the desktop currently targets. */
export const onDesktop = (settings: SepiaSettings, node: string | undefined): boolean =>
  nodeKey(node) === nodeKey(settings.desktop.node ?? undefined);

/** The desktop's agent pick — null when unset or `node` isn't the desktop's. */
export const desktopAgentFor = (
  settings: SepiaSettings,
  node: string | undefined,
): string | null => (onDesktop(settings, node) ? settings.desktop.agent : null);

/** The desktop's working dir — null when unset or `node` isn't the desktop's. */
export const desktopCwdFor = (settings: SepiaSettings, node: string | undefined): string | null =>
  onDesktop(settings, node) ? settings.desktop.cwd : null;

/**
 * The desktop's model pick for `agent` on `node` — the spawn-time model a
 * session of that agent inherits. `null` when unset, when `node` isn't the
 * desktop's machine, or when `agent` isn't the desktop's picked agent (a
 * desktop with no agent pick applies its model to whatever the node runs).
 */
export const desktopModelFor = (
  settings: SepiaSettings,
  node: string | undefined,
  agent: string,
): string | null =>
  onDesktop(settings, node) && (settings.desktop.agent === null || settings.desktop.agent === agent)
    ? settings.desktop.model
    : null;

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
