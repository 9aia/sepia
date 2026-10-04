import { Store } from "@tanstack/react-store";
import { pushRecent } from "./recents";

/**
 * Client-only UI state. Server state (sessions, history, agents, attach
 * results) lives in TanStack Query — see hooks/query/.
 */
export interface SepiaState {
  selectedId: string | null;
  /** Settings dialog — settingsSection scrolls it to a section on open. */
  settingsOpen: boolean;
  settingsSection: string | null;
  /** Session shown in the details drawer; rename opens its dialog. */
  detailsFor: { id: string; rename: boolean } | null;
  /** Pending cwd for "New session here" — consumed by SessionList. */
  createCwd: string | null;
  /** Working directory new sessions are created in; null = fall back to defaults. */
  cwd: string | null;
  /** Session a "New project" dialog should create-for and assign; null = closed. */
  newProjectFor: string | null;
}

export const sepiaStore = new Store<SepiaState>({
  selectedId: null,
  settingsOpen: false,
  settingsSection: null,
  detailsFor: null,
  createCwd: null,
  cwd: null,
  newProjectFor: null,
});

export const setSelectedId = (id: string | null): void => {
  if (id !== null) pushRecent(id);
  sepiaStore.setState((prev) => ({ ...prev, selectedId: id }));
};

export const setSettingsOpen = (open: boolean, section?: string): void => {
  sepiaStore.setState((prev) => ({
    ...prev,
    settingsOpen: open,
    settingsSection: open ? (section ?? null) : null,
  }));
};

export const setDetailsFor = (details: SepiaState["detailsFor"]): void => {
  sepiaStore.setState((prev) => ({ ...prev, detailsFor: details }));
};

export const setCwd = (cwd: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, cwd }));
};

export const setNewProjectFor = (sessionId: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, newProjectFor: sessionId }));
};

export const setCreateCwd = (cwd: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, createCwd: cwd }));
};
