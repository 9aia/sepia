import { Store } from "@tanstack/react-store";

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
  /** Agent id preselected when creating sessions; null = server default. */
  defaultAgent: string | null;
  /** cwd prefilled when creating sessions; null = most recent session's. */
  defaultCwd: string | null;
  /** Per-agent model prefs keyed by agent id. */
  models: Record<string, AgentModelPref>;
  /** Keybind overrides by action id — string = custom key, null = disabled. */
  keybinds: Record<string, string | null>;
  /** Browser push toggles — enabled = subscribed on this device. */
  notifications: { enabled: boolean; done: boolean; permission: boolean };
  /** UI theme — dark default; "system" follows prefers-color-scheme. */
  theme: "dark" | "light" | "system";
}

const KEY = "sepia:settings";

const load = (): SepiaSettings => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null)
      return {
        defaultAgent: null,
        defaultCwd: null,
        models: {},
        keybinds: {},
        notifications: { enabled: false, done: true, permission: true },
        theme: "dark",
      };
    const parsed = JSON.parse(raw) as Partial<SepiaSettings>;
    return {
      defaultAgent: typeof parsed.defaultAgent === "string" ? parsed.defaultAgent : null,
      defaultCwd: typeof parsed.defaultCwd === "string" ? parsed.defaultCwd : null,
      models:
        typeof parsed.models === "object" && parsed.models !== null
          ? (parsed.models as Record<string, AgentModelPref>)
          : {},
      keybinds:
        typeof parsed.keybinds === "object" && parsed.keybinds !== null
          ? (parsed.keybinds as Record<string, string | null>)
          : {},
      notifications:
        typeof parsed.notifications === "object" && parsed.notifications !== null
          ? {
              enabled: parsed.notifications.enabled === true,
              done: parsed.notifications.done !== false,
              permission: parsed.notifications.permission !== false,
            }
          : { enabled: false, done: true, permission: true },
      theme: parsed.theme === "light" || parsed.theme === "system" ? parsed.theme : "dark",
    };
  } catch {
    return {
      defaultAgent: null,
      defaultCwd: null,
      models: {},
      keybinds: {},
      notifications: { enabled: false, done: true, permission: true },
      theme: "dark",
    };
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
