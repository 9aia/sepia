import { Store } from "@tanstack/react-store";
import { getNode, listAgents, listProjects, listSessions, pairNode } from "./api";
import { LOCAL_NODE_ID, setLocalNodeAlias } from "./format";
import { createServer, deleteServer, gatewayTarget, updateServer } from "./servers";
import { localTarget, type ApiTarget } from "./targets";
import type { AgentInfo, NodeDescriptor, Project, SessionSummary } from "./types";

/**
 * The node registry (docs/protocol.md): the local node is implicit — it is
 * whichever server is serving this UI — and peers are `{url, token}` pairs
 * the user adds in Settings. Peers persist in localStorage; the rest of the
 * app reads `nodesStore.state` for fan-out and routing.
 */

export interface PeerNode {
  /** The peer's self-reported id from GET /api/node. */
  readonly id: string;
  readonly name: string;
  /** Origin of the peer's API, e.g. `https://thinkpad:8787` — no trailing slash. */
  readonly url: string;
  /**
   * The peer's bearer token — held by the browser for direct peers, null for
   * `via: "gateway"` peers (the credential lives server-side in the managed
   * registry and never enters the browser… beyond the one add-time submit).
   */
  readonly token: string | null;
  /**
   * "gateway" → this node's server forwards to the peer (docs/protocol.md
   * phase 3): the browser can't reach `url` directly, so calls go through
   * `/api/gateway/<serverId>` instead. Absent → direct browser→peer calls.
   */
  readonly via?: "gateway";
  /** Managed-server registry id holding the peer's url + credential. */
  readonly serverId?: string;
}

interface NodesState {
  /** The local node's /api/node descriptor, once `refreshSelf` has run. */
  readonly self: NodeDescriptor | null;
  readonly peers: ReadonlyArray<PeerNode>;
}

/** Per-peer fan-out timeout — a dead laptop must not stall the merged list. */
const PEER_TIMEOUT_MS = 3_000;
/** Add-node validation gets a slightly longer leash (user is waiting on it). */
const PROBE_TIMEOUT_MS = 5_000;

const PEERS_KEY = "sepia:nodes";

/** Lenient stored-record read — a malformed entry is dropped, not fatal. */
export const normalizePeer = (value: unknown): PeerNode | null => {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "string" || raw.id === "") return null;
  if (typeof raw.url !== "string" || raw.url === "") return null;
  // A gateway peer needs its managed-registry id to resolve a target; a
  // stored entry that lost it degrades to direct (calls will just fail).
  const gateway = raw.via === "gateway" && typeof raw.serverId === "string" && raw.serverId !== "";
  return {
    id: raw.id,
    name: typeof raw.name === "string" && raw.name !== "" ? raw.name : raw.url,
    url: raw.url,
    token: typeof raw.token === "string" && raw.token !== "" ? raw.token : null,
    ...(gateway ? { via: "gateway" as const, serverId: raw.serverId as string } : {}),
  };
};

const loadPeers = (): PeerNode[] => {
  try {
    const raw = localStorage.getItem(PEERS_KEY);
    if (raw === null) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.map(normalizePeer).filter((peer): peer is PeerNode => peer !== null);
  } catch {
    return [];
  }
};

const persistPeers = (peers: ReadonlyArray<PeerNode>): void => {
  try {
    localStorage.setItem(PEERS_KEY, JSON.stringify(peers));
  } catch {
    // Storage unavailable — peers live for the session.
  }
};

export const nodesStore = new Store<NodesState>({ self: null, peers: loadPeers() });

/** Registered peers, sorted by name for stable display. */
export const getPeers = (): ReadonlyArray<PeerNode> => nodesStore.state.peers;

/** Whether any peer is registered — gates every federation UI affordance. */
export const isMultiNode = (): boolean => nodesStore.state.peers.length > 0;

/** Normalize a user-entered address to an origin the API calls can prefix. */
export const normalizeNodeUrl = (input: string): string => {
  const trimmed = input.trim().replace(/\/+$/, "");
  const withScheme = /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
  const url = new URL(withScheme);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("Node address must be an http(s) URL");
  }
  return url.origin;
};

/**
 * Insert or replace a peer by its server id (re-adding an already-registered
 * node refreshes its url/token/name). Returns the new list — pure so the
 * registry logic is testable without a Store.
 */
export const upsertPeer = (peers: ReadonlyArray<PeerNode>, peer: PeerNode): PeerNode[] => {
  const rest = peers.filter((p) => p.id !== peer.id && p.url !== peer.url);
  return [...rest, peer];
};

export const removePeerById = (peers: ReadonlyArray<PeerNode>, id: string): PeerNode[] =>
  peers.filter((p) => p.id !== id);

const commitPeers = (peers: ReadonlyArray<PeerNode>): void => {
  persistPeers(peers);
  nodesStore.setState((prev) => ({ ...prev, peers }));
};

/** Fetch + validate a peer, then register it. Throws on unreachable/401. */
export const addPeer = async (url: string, token: string): Promise<PeerNode> => {
  const target: ApiTarget = {
    baseUrl: normalizeNodeUrl(url),
    token: token.trim() === "" ? null : token.trim(),
    timeoutMs: PROBE_TIMEOUT_MS,
  };
  const descriptor = await getNode(target);
  const self = nodesStore.state.self;
  if (self !== null && descriptor.id === self.id) {
    throw new Error("That's this machine — it's already in the list");
  }
  const peer: PeerNode = {
    id: descriptor.id,
    name: descriptor.name,
    url: target.baseUrl,
    token: target.token,
  };
  commitPeers(upsertPeer(nodesStore.state.peers, peer));
  return peer;
};

export const removePeer = (id: string): void => {
  commitPeers(removePeerById(nodesStore.state.peers, id));
};

/**
 * Remove a peer and, for `via: "gateway"` peers, the managed-server entry
 * holding its credential. The managed delete is best-effort — an already-gone
 * entry (removed via Settings → Servers) must not strand the peer row.
 */
export const removePeerEntry = async (id: string): Promise<void> => {
  const peer = nodesStore.state.peers.find((p) => p.id === id);
  if (peer?.via === "gateway" && peer.serverId !== undefined) {
    await deleteServer(peer.serverId).catch(() => undefined);
  }
  removePeer(id);
};

/**
 * The pairing add-path (docs/protocol.md): redeem the one-time code `sepia
 * pair` printed on the node for a long-lived credential, then register the
 * peer exactly like the manual token flow.
 */
export const pairPeer = async (url: string, code: string): Promise<PeerNode> => {
  const baseUrl = normalizeNodeUrl(url);
  const { token } = await pairNode(code.trim(), {
    baseUrl,
    token: null,
    timeoutMs: PROBE_TIMEOUT_MS,
  });
  return addPeer(baseUrl, token);
};

// --- Gateway-mode peers (docs/protocol.md phase 3) ---------------------------
//
// A `via: "gateway"` peer keeps no credential in the browser: the add flow
// registers the peer in this node's managed-server registry (`/api/servers`,
// encrypted at rest) and every call rides `ANY /api/gateway/<serverId>`,
// where the server injects the stored credential. Probing goes *through* the
// gateway — a peer the browser can't reach directly is exactly the case
// gateway mode exists for, so direct probes would always fail.

/** url → the `{scheme, host, port}` triple the managed registry stores. */
const gatewayTargetParts = (
  baseUrl: string,
): { scheme: "http" | "https"; host: string; port: number } => {
  const url = new URL(baseUrl);
  const scheme = url.protocol === "https:" ? "https" : "http";
  const port = url.port === "" ? (scheme === "https" ? 443 : 80) : Number(url.port);
  return { scheme, host: url.hostname, port };
};

/**
 * Probe the peer through the fresh gateway entry and register it. Throws (and
 * the caller drops the managed entry) when the peer doesn't answer, answers
 * 401, or turns out to be this machine.
 */
const registerGatewayPeer = async (baseUrl: string, serverId: string): Promise<PeerNode> => {
  const descriptor = await getNode({ ...gatewayTarget(serverId), timeoutMs: PROBE_TIMEOUT_MS });
  const self = nodesStore.state.self;
  if (self !== null && descriptor.id === self.id) {
    throw new Error("That's this machine — it's already in the list");
  }
  const peer: PeerNode = {
    id: descriptor.id,
    name: descriptor.name,
    url: baseUrl,
    token: null,
    via: "gateway",
    serverId,
  };
  commitPeers(upsertPeer(nodesStore.state.peers, peer));
  return peer;
};

/**
 * Token add-path for a gateway peer: the submitted token goes to the managed
 * registry (it never persists in the browser), then the peer is probed and
 * registered through the gateway. On failure the managed entry is dropped so
 * a rejected add leaves no orphaned credential behind.
 */
export const addGatewayPeer = async (url: string, token: string): Promise<PeerNode> => {
  const baseUrl = normalizeNodeUrl(url);
  const { scheme, host, port } = gatewayTargetParts(baseUrl);
  const secret = token.trim();
  const entry = await createServer({
    label: host,
    host,
    port,
    scheme,
    auth: secret === "" ? null : { type: "token", secret },
    ssh: null,
  });
  try {
    return await registerGatewayPeer(baseUrl, entry.id);
  } catch (error) {
    await deleteServer(entry.id).catch(() => undefined);
    throw error;
  }
};

/**
 * Pairing add-path through the gateway: the managed entry starts credential-
 * less (POST /api/pair is unauthenticated on the peer), the code is redeemed
 * through the fresh gateway hop — the local node's bearer authorizes that
 * forward — and the issued token is written back into the registry.
 */
export const pairGatewayPeer = async (url: string, code: string): Promise<PeerNode> => {
  const baseUrl = normalizeNodeUrl(url);
  const { scheme, host, port } = gatewayTargetParts(baseUrl);
  const entry = await createServer({ label: host, host, port, scheme, auth: null, ssh: null });
  try {
    const { token } = await pairNode(code.trim(), gatewayTarget(entry.id), {
      forwardTargetAuth: true,
    });
    await updateServer(entry.id, {
      label: host,
      host,
      port,
      scheme,
      auth: { type: "token", secret: token },
      ssh: null,
    });
    return await registerGatewayPeer(baseUrl, entry.id);
  } catch (error) {
    await deleteServer(entry.id).catch(() => undefined);
    throw error;
  }
};

/** Populate `nodesStore.self` from the local node's own /api/node. */
export const refreshSelf = async (): Promise<NodeDescriptor> => {
  const descriptor = await getNode(localTarget());
  setLocalNodeAlias(descriptor.id);
  nodesStore.setState((prev) => ({ ...prev, self: descriptor }));
  return descriptor;
};

/**
 * API target for a registered peer. A `via: "gateway"` peer resolves to this
 * node's `/api/gateway/<serverId>` forward — every call site (fan-out lists,
 * session actions, SSE streams) routes through it unchanged; a direct peer
 * resolves to its own origin + browser-held token.
 */
export const peerTarget = (peer: PeerNode): ApiTarget => {
  if (peer.via === "gateway" && peer.serverId !== undefined) {
    return gatewayTarget(peer.serverId);
  }
  return { baseUrl: peer.url, token: peer.token, timeoutMs: PEER_TIMEOUT_MS };
};

/**
 * Resolve a row's `node` field to the API target that owns it. Local rows
 * (undefined/"local") hit the same-origin server; an unknown peer id yields
 * a deliberately unreachable target so stale rows fail instead of silently
 * mutating the local machine.
 */
export const nodeTarget = (node: string | undefined): ApiTarget => {
  if (node === undefined || node === LOCAL_NODE_ID) return localTarget();
  const peer = nodesStore.state.peers.find((p) => p.id === node);
  if (peer !== undefined) return peerTarget(peer);
  return { baseUrl: `http://${node}.invalid`, token: null, timeoutMs: PEER_TIMEOUT_MS };
};

/** Display name for a node id — local resolves to the machine's hostname. */
export const nodeName = (node: string | undefined): string => {
  if (node === undefined || node === LOCAL_NODE_ID) {
    return nodesStore.state.self?.name ?? "this machine";
  }
  return nodesStore.state.peers.find((p) => p.id === node)?.name ?? node;
};

/**
 * Label for a run span's node. Spans store the server's real node id (not
 * the `"local"` key alias), so resolve against `self` first, then the peer
 * registry; an unknown id falls back to a truncated prefix, and the local
 * sentinel — recorded before the identity resolved — reads "local".
 */
export const spanNodeLabel = (node: string): string => {
  const self = nodesStore.state.self;
  if (node === LOCAL_NODE_ID || (self !== null && node === self.id)) {
    return self?.name ?? "local";
  }
  const peer = nodesStore.state.peers.find((p) => p.id === node);
  if (peer !== undefined) return peer.name;
  if (node === "") return "local";
  return node.length > 14 ? `${node.slice(0, 14)}…` : node;
};

// --- Fan-out fetches ---------------------------------------------------------
//
// Each merged list calls every registered node. The LOCAL node's failure is
// fatal (it preserves today's error/TokenGate UX); a peer's failure means
// that node simply contributes nothing (docs/protocol.md failure model).

const tagSession = (session: SessionSummary, node: string | undefined): SessionSummary =>
  node === undefined
    ? session
    : {
        ...session,
        node,
        // Project references are node-local too — namespace them so they
        // match the merged projects collection's `node:id` keys.
        projectIds: session.projectIds.map((id) => `${node}:${id}`),
      };

const tagProject = (project: Project, node: string | undefined): Project =>
  node === undefined ? project : { ...project, node };

export const listAllSessions = async (): Promise<SessionSummary[]> => {
  const peers = nodesStore.state.peers;
  const multi = peers.length > 0;
  const local = await listSessions(localTarget());
  const rows = local.map((s) => tagSession(s, multi ? LOCAL_NODE_ID : undefined));
  if (!multi) return rows;
  const settled = await Promise.allSettled(
    peers.map((peer) =>
      listSessions(peerTarget(peer)).then((list) => list.map((s) => tagSession(s, peer.id))),
    ),
  );
  for (const result of settled) {
    if (result.status === "fulfilled") rows.push(...result.value);
  }
  return rows;
};

export const listAllProjects = async (): Promise<Project[]> => {
  const peers = nodesStore.state.peers;
  const multi = peers.length > 0;
  const local = await listProjects(localTarget());
  const rows = local.projects.map((p) => tagProject(p, multi ? LOCAL_NODE_ID : undefined));
  if (!multi) return rows;
  const settled = await Promise.allSettled(
    peers.map((peer) =>
      listProjects(peerTarget(peer)).then((data) =>
        data.projects.map((p) => tagProject(p, peer.id)),
      ),
    ),
  );
  for (const result of settled) {
    if (result.status === "fulfilled") rows.push(...result.value);
  }
  return rows;
};

/** Union of agent rosters across nodes — deduped by agent id, local wins. */
export const listAllAgents = async (): Promise<AgentInfo[]> => {
  const peers = nodesStore.state.peers;
  const merged = new Map<string, AgentInfo>();
  for (const agent of await listAgents(localTarget())) merged.set(agent.id, agent);
  if (peers.length === 0) return [...merged.values()];
  const settled = await Promise.allSettled(peers.map((peer) => listAgents(peerTarget(peer))));
  for (const result of settled) {
    if (result.status === "fulfilled") {
      for (const agent of result.value) {
        if (!merged.has(agent.id)) merged.set(agent.id, agent);
      }
    }
  }
  return [...merged.values()];
};
