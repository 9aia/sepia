import type { Query, QueryClient } from "@tanstack/react-query";
import { queryKeys } from "../hooks/query/keys";
import { LOCAL_NODE_ID } from "./format";
import { nodesStore, peerTarget } from "./nodes";
import { localTarget, type ApiTarget } from "./targets";
import { onTokenChange } from "./token";

/**
 * The node event feed (docs/protocol.md "GET /api/events"): one SSE stream
 * per registered node — the local machine plus every peer. Events are
 * invalidation hints: the merged fan-out queries refetch rather than the UI
 * patching rows from payloads (a missed event just means a stale row until
 * the next refetch or reconnect).
 */
export interface NodeEvent {
  readonly kind: string;
  readonly id?: string;
  readonly agent?: string;
  readonly patch?: Record<string, unknown>;
}

/**
 * Query keys a feed event invalidates. `session` and `meta` both touch the
 * merged session list (meta fields ride on the summary rows); `project`
 * touches the merged project list. Pure — the subscription maps every event
 * through this.
 */
export const invalidationsForEvent = (event: NodeEvent): ReadonlyArray<readonly unknown[]> => {
  switch (event.kind) {
    case "session":
    case "meta":
      return [queryKeys.sessions];
    case "project":
      return [queryKeys.projects];
    default:
      // heartbeat and unknown kinds — no cache work.
      return [];
  }
};

/**
 * History keys are `id`, `agent:id`, or `node:agent:id` — the session id is
 * always the tail segment, so a suffix match covers every form across nodes.
 */
export const historyKeyMatches = (key: unknown, id: string): boolean =>
  typeof key === "string" && (key === id || key.endsWith(`:${id}`));

const historyPredicate =
  (id: string) =>
  (query: Query): boolean =>
    historyKeyMatches(query.queryKey[1], id);

/** Apply one feed event to the cache — invalidates the matching queries. */
export const applyNodeEvent = (client: QueryClient, event: NodeEvent): void => {
  for (const queryKey of invalidationsForEvent(event)) {
    void client.invalidateQueries({ queryKey });
  }
  // A summary row changed — if that session's transcript is open, re-sync it
  // too (a run finishing flushes IR into the store, same as the live stream's
  // RUN_FINISHED invalidation). The predicate scopes to the event's id.
  if (event.kind === "session" && typeof event.id === "string" && event.id !== "") {
    void client.invalidateQueries({
      queryKey: ["history"],
      predicate: historyPredicate(event.id),
    });
  }
};

const FEED_KINDS = ["session", "meta", "project"] as const;

/**
 * One /api/events EventSource against `target`. EventSource reconnects with
 * its own backoff on network gaps; the stream stays silent — the
 * per-session reconnect pill is a different surface (see StreamStatus).
 */
export const subscribeNodeFeed = (
  target: ApiTarget,
  onEvent: (event: NodeEvent) => void,
): (() => void) => {
  if (typeof EventSource === "undefined") return () => {};
  const params = new URLSearchParams();
  if (target.token !== null) params.set("access_token", target.token);
  const query = params.size === 0 ? "" : `?${params.toString()}`;
  const source = new EventSource(`${target.baseUrl}/api/events${query}`);
  for (const kind of FEED_KINDS) {
    source.addEventListener(kind, (message) => {
      try {
        const data = JSON.parse((message as MessageEvent).data) as Record<string, unknown>;
        onEvent({ kind, ...data });
      } catch {
        // Malformed frame — best effort, like the per-session stream.
      }
    });
  }
  return () => source.close();
};

interface OpenFeed {
  /** Node id + credential — a token change re-subscribes the feed. */
  readonly signature: string;
  readonly close: () => void;
}

const signature = (node: string, target: ApiTarget): string => `${node} ${target.token ?? ""}`;

/**
 * Keeps one feed open per registered node: the local node (same origin)
 * plus each peer in `nodesStore`. Re-syncs when the registry or the local
 * token changes; returns a stop that closes every stream.
 */
export const startNodeEventFeeds = (client: QueryClient): (() => void) => {
  const feeds = new Map<string, OpenFeed>();
  const sync = (): void => {
    const wanted = new Map<string, ApiTarget>();
    wanted.set(LOCAL_NODE_ID, localTarget());
    for (const peer of nodesStore.state.peers) {
      wanted.set(peer.id, peerTarget(peer));
    }
    for (const [node, feed] of feeds) {
      const target = wanted.get(node);
      if (target === undefined || feed.signature !== signature(node, target)) {
        feed.close();
        feeds.delete(node);
      }
    }
    for (const [node, target] of wanted) {
      if (feeds.has(node)) continue;
      feeds.set(node, {
        signature: signature(node, target),
        close: subscribeNodeFeed(target, (event) => applyNodeEvent(client, event)),
      });
    }
  };
  sync();
  const storeSub = nodesStore.subscribe(() => sync());
  const tokenSub = onTokenChange(sync);
  return () => {
    storeSub.unsubscribe();
    tokenSub();
    for (const feed of feeds.values()) feed.close();
    feeds.clear();
  };
};
