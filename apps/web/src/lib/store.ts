import { Store } from "@tanstack/react-store";

/**
 * Client-only UI state. Server state (sessions, history, agents, attach
 * results) lives in TanStack Query — see hooks/query/.
 */
export interface SepiaState {
  selectedId: string | null;
  keybindsOpen: boolean;
  /** Session shown in the details drawer; rename focuses the title field. */
  detailsFor: { id: string; rename: boolean } | null;
}

export const sepiaStore = new Store<SepiaState>({
  selectedId: null,
  keybindsOpen: false,
  detailsFor: null,
});

export const setSelectedId = (id: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, selectedId: id }));
};

export const setKeybindsOpen = (open: boolean): void => {
  sepiaStore.setState((prev) => ({ ...prev, keybindsOpen: open }));
};

export const setDetailsFor = (details: SepiaState["detailsFor"]): void => {
  sepiaStore.setState((prev) => ({ ...prev, detailsFor: details }));
};
