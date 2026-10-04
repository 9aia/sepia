import { Store } from "@tanstack/react-store";
import { pushRecent } from "./recents";

/**
 * Client-only UI state. Server state (sessions, history, agents, attach
 * results) lives in TanStack Query — see hooks/query/.
 */
export interface SepiaState {
  selectedId: string | null;
  keybindsOpen: boolean;
  /** Session shown in the details drawer; rename opens its dialog. */
  detailsFor: { id: string; rename: boolean } | null;
  /** Pending cwd for "New session here" — consumed by SessionList. */
  createCwd: string | null;
  /** Working directory new sessions are created in; null = fall back to defaults. */
  cwd: string | null;
}

export const sepiaStore = new Store<SepiaState>({
  selectedId: null,
  keybindsOpen: false,
  detailsFor: null,
  createCwd: null,
  cwd: null,
});

export const setSelectedId = (id: string | null): void => {
  if (id !== null) pushRecent(id);
  sepiaStore.setState((prev) => ({ ...prev, selectedId: id }));
};

export const setKeybindsOpen = (open: boolean): void => {
  sepiaStore.setState((prev) => ({ ...prev, keybindsOpen: open }));
};

export const setDetailsFor = (details: SepiaState["detailsFor"]): void => {
  sepiaStore.setState((prev) => ({ ...prev, detailsFor: details }));
};

export const setCwd = (cwd: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, cwd }));
};

export const setCreateCwd = (cwd: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, createCwd: cwd }));
};
