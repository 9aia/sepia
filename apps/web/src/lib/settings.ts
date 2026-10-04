import { Store } from "@tanstack/react-store";

/**
 * Client-side preferences, persisted to localStorage. Server-side
 * configuration (agents, spawn env) stays in `apps/server` env vars — these
 * are per-browser UI defaults only.
 */
export interface SepiaSettings {
  /** Agent id preselected when creating sessions; null = server default. */
  defaultAgent: string | null;
  /** cwd prefilled when creating sessions; null = most recent session's. */
  defaultCwd: string | null;
}

const KEY = "sepia:settings";

const load = (): SepiaSettings => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return { defaultAgent: null, defaultCwd: null };
    const parsed = JSON.parse(raw) as Partial<SepiaSettings>;
    return {
      defaultAgent: typeof parsed.defaultAgent === "string" ? parsed.defaultAgent : null,
      defaultCwd: typeof parsed.defaultCwd === "string" ? parsed.defaultCwd : null,
    };
  } catch {
    return { defaultAgent: null, defaultCwd: null };
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
