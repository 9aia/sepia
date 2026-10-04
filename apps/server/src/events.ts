import type { MetaStore, SessionMeta } from "./meta";

/**
 * The node event feed behind `GET /api/events` (docs/protocol.md): one
 * process-wide emitter that route handlers and the meta overlay write into,
 * and SSE connections drain. Events are hints — a missed one just means a
 * stale row until the next refetch, so subscriber queues are bounded and
 * drop the oldest events rather than back-pressure the emitters.
 */
export type NodeEventKind = "session" | "meta" | "project";

export interface NodeEvent {
  readonly kind: NodeEventKind;
  readonly payload: Record<string, unknown>;
}

/** `{"id","agent","patch"}` — the wire shape of session/meta events. */
export const sessionPayload = (
  id: string,
  agent: string | undefined,
  patch: Record<string, unknown>,
): Record<string, unknown> => ({ id, ...(agent === undefined ? {} : { agent }), patch });

/**
 * Bound per subscriber. A client this far behind is resyncing anyway
 * (invalidations collapse), so the oldest events are the cheapest to drop.
 */
const MAX_QUEUED = 500;

interface Subscription {
  queue: NodeEvent[];
  waiter: ((event: NodeEvent | undefined) => void) | null;
  closed: boolean;
}

export interface EventSubscription {
  /** Next queued event; `undefined` once closed and drained. */
  readonly next: () => Promise<NodeEvent | undefined>;
  readonly close: () => void;
}

export interface NodeEventFeed {
  readonly emit: (kind: NodeEventKind, payload: Record<string, unknown>) => void;
  readonly subscribe: () => EventSubscription;
}

export const createEventFeed = (): NodeEventFeed => {
  const subscribers = new Set<Subscription>();
  return {
    emit: (kind, payload) => {
      const event: NodeEvent = { kind, payload };
      for (const sub of subscribers) {
        if (sub.waiter !== null) {
          const waiter = sub.waiter;
          sub.waiter = null;
          waiter(event);
          continue;
        }
        if (sub.queue.length >= MAX_QUEUED) sub.queue.shift();
        sub.queue.push(event);
      }
    },
    subscribe: () => {
      const sub: Subscription = { queue: [], waiter: null, closed: false };
      subscribers.add(sub);
      return {
        next: () => {
          const queued = sub.queue.shift();
          if (queued !== undefined) return Promise.resolve(queued);
          if (sub.closed) return Promise.resolve(undefined);
          return new Promise<NodeEvent | undefined>((resolve) => {
            sub.waiter = resolve;
          });
        },
        close: () => {
          if (sub.closed) return;
          sub.closed = true;
          subscribers.delete(sub);
          if (sub.waiter !== null) {
            const waiter = sub.waiter;
            sub.waiter = null;
            waiter(undefined);
          }
        },
      };
    },
  };
};

/** The session row's `busy` flag mirrors the live turn lifecycle. */
export const busyFromEvents = (
  events: ReadonlyArray<{ readonly type?: string }>,
): boolean | undefined => {
  // Both edges can land in one batch (load-failure unwind); the run is over.
  if (events.some((event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")) {
    return false;
  }
  if (events.some((event) => event.type === "RUN_STARTED")) return true;
  return undefined;
};

const metaPatchPayload = (
  meta: MetaStore,
  id: string,
  patch: Record<string, unknown>,
): Record<string, unknown> => sessionPayload(id, meta.of(id)?.agent, patch);

/**
 * Wraps a MetaStore so every overlay write also lands on the node feed:
 * session-meta writes emit `meta`, project writes emit `project`. Reads and
 * `setConfig` pass through untouched — config isn't part of the feed.
 */
export const instrumentMeta = (meta: MetaStore, feed: NodeEventFeed): MetaStore => ({
  of: meta.of,
  sessions: meta.sessions,
  patch: (id: string, patch: Partial<SessionMeta>) => {
    meta.patch(id, patch);
    feed.emit("meta", metaPatchPayload(meta, id, { ...patch }));
  },
  addSpan: (id, span) => {
    const before = meta.of(id)?.spans;
    meta.addSpan(id, span);
    const after = meta.of(id)?.spans;
    // appendSpan is idempotent — a same-agent+node re-attach is a no-op.
    if (after !== before) {
      feed.emit("meta", metaPatchPayload(meta, id, { spans: after ?? [] }));
    }
  },
  remove: (id) => {
    const existing = meta.of(id);
    meta.remove(id);
    if (existing !== undefined) {
      feed.emit("meta", sessionPayload(id, existing.agent, { deleted: true }));
    }
  },
  listProjects: meta.listProjects,
  createProject: (name) => {
    const project = meta.createProject(name);
    feed.emit("project", { id: project.id, patch: { name: project.name } });
    return project;
  },
  renameProject: (id, name) => {
    const renamed = meta.renameProject(id, name);
    if (renamed) feed.emit("project", { id, patch: { name } });
    return renamed;
  },
  deleteProject: (id) => {
    const existed = meta.listProjects().some((project) => project.id === id);
    meta.deleteProject(id);
    if (existed) feed.emit("project", { id, patch: { deleted: true } });
  },
  config: meta.config,
  setConfig: meta.setConfig,
});
