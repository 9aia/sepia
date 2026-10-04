import { Store } from "@tanstack/react-store";

/**
 * Client-only UI state. Server state (sessions, history, agents, attach
 * results) lives in TanStack Query — see hooks/query/.
 */
export interface SepiaState {
  selectedId: string | null;
  keybindsOpen: boolean;
}

export const sepiaStore = new Store<SepiaState>({ selectedId: null, keybindsOpen: false });

export const setSelectedId = (id: string | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, selectedId: id }));
};

export const setKeybindsOpen = (open: boolean): void => {
  sepiaStore.setState((prev) => ({ ...prev, keybindsOpen: open }));
};
