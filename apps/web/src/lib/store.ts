import { Store } from "@tanstack/react-store";
import { attach, createSession, deleteSession, getHistory, listAgents, listSessions } from "./api";
import type { AgentInfo, CreateSessionInput, HistoryMessage, SessionSummary } from "./types";

export interface SepiaState {
  sessions: SessionSummary[];
  agents: AgentInfo[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
  creating: boolean;
  createError: string | null;
  history: HistoryMessage[];
  historyTotal: number;
  readOnly: boolean;
  attachError: string | null;
  historyError: string | null;
}

export const sepiaStore = new Store<SepiaState>({
  sessions: [],
  agents: [],
  selectedId: null,
  loading: false,
  error: null,
  creating: false,
  createError: null,
  history: [],
  historyTotal: 0,
  readOnly: false,
  attachError: null,
  historyError: null,
});

const patch = (update: Partial<SepiaState>): void => {
  sepiaStore.setState((prev) => ({ ...prev, ...update }));
};

const messageOf = (err: unknown, fallback: string): string =>
  err instanceof Error ? err.message : fallback;

/** True when `id` is still the selected session; guards stale async writes. */
const stillSelected = (id: string): boolean => sepiaStore.state.selectedId === id;

export const selectSession = (id: string): void => {
  patch({ selectedId: id, readOnly: false, attachError: null, historyError: null });
  attach(id)
    .then((result) => {
      if (stillSelected(id)) patch({ readOnly: result.readOnly });
    })
    .catch((err: unknown) => {
      if (stillSelected(id))
        patch({ readOnly: true, attachError: messageOf(err, "Failed to attach session") });
    });
  getHistory(id)
    .then((data) => {
      if (stillSelected(id)) patch({ history: data.messages, historyTotal: data.total });
    })
    .catch((err: unknown) => {
      if (stillSelected(id))
        patch({
          history: [],
          historyTotal: 0,
          historyError: messageOf(err, "Failed to load history"),
        });
    });
};

export const loadAgents = (): void => {
  listAgents()
    .then((agents) => patch({ agents }))
    .catch(() => undefined);
};

/** Refetches the stored backlog for `id` (e.g. after a run finishes). */
export const refreshHistory = (id: string): void => {
  getHistory(id)
    .then((data) => {
      if (stillSelected(id)) patch({ history: data.messages, historyTotal: data.total });
    })
    .catch(() => undefined);
};

export const refreshSessions = (): void => {
  listSessions()
    .then((data) => {
      patch({ sessions: data, loading: false, error: null });
      // Auto-select the first session only when nothing is selected.
      const first = data[0];
      if (sepiaStore.state.selectedId === null && first !== undefined) {
        selectSession(first.id);
      }
    })
    .catch((err: unknown) => {
      patch({ loading: false, error: messageOf(err, "Failed to list sessions") });
    });
};

export const createNewSession = (input: CreateSessionInput): void => {
  patch({ creating: true, createError: null });
  createSession(input)
    .then(({ id }) => {
      patch({ creating: false });
      refreshSessions();
      selectSession(id);
    })
    .catch((err: unknown) => {
      patch({ creating: false, createError: messageOf(err, "Failed to create session") });
    });
};

export const removeSession = (id: string): void => {
  deleteSession(id)
    .then(() => {
      patch({ sessions: sepiaStore.state.sessions.filter((s) => s.id !== id) });
      if (stillSelected(id)) {
        patch({
          selectedId: null,
          history: [],
          historyTotal: 0,
          readOnly: false,
          attachError: null,
          historyError: null,
        });
      }
    })
    .catch((err: unknown) => {
      patch({ error: messageOf(err, "Failed to delete session") });
    });
};

export const takeoverSession = (id: string): void => {
  patch({ attachError: null });
  attach(id, { takeover: true })
    .then((result) => {
      if (stillSelected(id)) patch({ readOnly: result.readOnly });
      refreshSessions();
    })
    .catch((err: unknown) => {
      patch({ attachError: messageOf(err, "Takeover failed") });
    });
};
