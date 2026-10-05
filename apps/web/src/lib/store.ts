import { Store } from "@tanstack/react-store";
import { pushRecent } from "./recents";
import type { ReplyQuote } from "./reply";

/**
 * The sidebar's explicit create target — which machine+agent "New session"
 * aims at. `node` is a nodeKey ("local" or a registered peer id); `agent`
 * is an explicit agent pick, null = the node's own default. A null focus
 * means "no override" — creates fall back to per-node settings defaults.
 */
export interface FocusTarget {
  readonly node: string;
  readonly agent: string | null;
}

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
  /** Node the pending "New session here" targets; null = this machine. */
  createNode: string | null;
  /** Working directory new sessions are created in; null = fall back to defaults. */
  cwd: string | null;
  /** Session a "New project" dialog should create-for and assign; null = closed. */
  newProjectFor: string | null;
  /** Message the composer is quoting; cleared on session switch and on send. */
  replyTo: ReplyQuote | null;
  /** Focused create target (ClientBar picks it); null = per-node defaults. */
  focus: FocusTarget | null;
}

export const sepiaStore = new Store<SepiaState>({
  selectedId: null,
  settingsOpen: false,
  settingsSection: null,
  detailsFor: null,
  createCwd: null,
  createNode: null,
  cwd: null,
  newProjectFor: null,
  replyTo: null,
  focus: null,
});

export const setSelectedId = (id: string | null): void => {
  if (id !== null) pushRecent(id);
  sepiaStore.setState((prev) =>
    prev.selectedId === id ? prev : { ...prev, selectedId: id, replyTo: null },
  );
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

export const setCreateCwd = (cwd: string | null, node?: string): void => {
  sepiaStore.setState((prev) => ({
    ...prev,
    createCwd: cwd,
    createNode: cwd === null ? null : (node ?? null),
  }));
};

export const setReplyTo = (replyTo: ReplyQuote | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, replyTo }));
};

export const setFocus = (focus: FocusTarget | null): void => {
  sepiaStore.setState((prev) => ({ ...prev, focus }));
};
