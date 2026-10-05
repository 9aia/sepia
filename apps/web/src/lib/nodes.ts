import { Store } from "@tanstack/react-store";
import { getNode, listAgents, listProjects, listSessions, pairNode } from "./api";
import { addCredential, credentialById } from "./credentials";
import { LOCAL_NODE_ID, setLocalNodeAlias } from "./format";
import {
  createServer,
  deleteServer,
  gatewayTarget,
  listServers,
  SECRET_MASK,
  updateServer,
} from "./servers";
import { setSettings, settingsStore } from "./settings";
import { localTarget, type ApiTarget } from "./targets";
import type { AgentInfo, NodeDescriptor, Project, SessionSummary } from "./types";

/**
 * The node registry (docs/protocol.md): the local node is implicit — it is
 * whichever server is serving this UI — and peers are `{url, credentialId}`
 * pairs the user adds in Settings. Peers persist in localStorage; the rest
 * of the app reads `nodesStore.state` for fan-out and routing.
 */

export interface PeerNode {
  /** The peer's self-reported id from GET /api/node. */
  readonly id: string;
  readonly name: string;
  /** User-assigned nickname — overrides `name` everywhere the node displays. */
  readonly alias?: string;
  /** Origin of the peer's API, e.g. `https://thinkpad:8787` — no trailing slash. */
  readonly url: string;
  /**
   * The peer's credential — a reference into the browser credential store
   * (lib/credentials.ts) for direct peers, absent for `via: "gateway"`
   * peers (the credential lives server-side in the managed registry and
   * never enters the browser… beyond the one add-time submit). A dangling
   * id — its credential was deleted — resolves to no auth, so the peer's
   * calls fail instead of silently sending a stale secret.
   */
  readonly credentialId?: string;
  /**
   * "gateway" → this node's server forwards to the peer (docs/protocol.md
   * phase 3): the browser can't reach `url` directly, so calls go through
   * `/api/gateway/<serverId>` instead. Absent → direct browser→peer calls.
   */
  readonly via?: "gateway";
  /** Managed-server registry id holding the peer's url + credential. */
  readonly serverId?: string;
  /**
   * Settings → Nodes' enable switch. Absent means enabled; `false` parks the
   * peer — it stays registered (and editable) but contributes nothing to
   * fan-out lists, status probes, event feeds or resume targets, and
   * `peerTarget`/`nodeTarget` resolve it to the unreachable sentinel.
   */
  readonly enabled?: boolean;
}

/** Local-node reachability — "unknown" until the first probe settles. */
export type SelfStatus = "unknown" | "online" | "offline";

interface NodesState {
  /** The local node's /api/node descriptor, once `refreshSelf` has run. */
  readonly self: NodeDescriptor | null;
  /**
   * This machine's reachability — the same degradation peers get via
   * `useNodeStatuses`, surfaced on the store so the sidebar, node badges
   * and Settings → Nodes can mark this machine offline. Any local probe
   * (a fan-out leg or `refreshSelf`) can move it.
   */
  readonly selfStatus: SelfStatus;
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
    // A legacy inline `token` is deliberately not read here — loadPeers
    // upgrades it into the credential store (see upgradeLegacyToken).
    ...(typeof raw.credentialId === "string" && raw.credentialId !== ""
      ? { credentialId: raw.credentialId }
      : {}),
    ...(typeof raw.alias === "string" && raw.alias !== "" ? { alias: raw.alias } : {}),
    // Absent (and any non-false legacy value) reads as enabled.
    ...(raw.enabled === false ? { enabled: false as const } : {}),
    ...(gateway ? { via: "gateway" as const, serverId: raw.serverId as string } : {}),
  };
};

/**
 * One-time upgrade for pre-credentials peer records: an inline `token`
 * becomes a managed credential named after the peer, linked by
 * `credentialId`, and the raw secret never persists on the peer again.
 * Gateway peers are skipped — their credential was never browser-held.
 */
const upgradeLegacyToken = (record: unknown, peer: PeerNode): PeerNode => {
  if (typeof record !== "object" || record === null) return peer;
  if (peer.credentialId !== undefined || peer.via === "gateway") return peer;
  const token = (record as Record<string, unknown>).token;
  if (typeof token !== "string" || token === "") return peer;
  const credential = addCredential({ label: peer.alias ?? peer.name, secret: token });
  return { ...peer, credentialId: credential.id };
};

const loadPeers = (): PeerNode[] => {
  try {
    const raw = localStorage.getItem(PEERS_KEY);
    if (raw === null) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    let migrated = false;
    const peers = parsed
      .map((record) => {
        const peer = normalizePeer(record);
        if (peer === null) return null;
        const upgraded = upgradeLegacyToken(record, peer);
        if (upgraded !== peer) migrated = true;
        return upgraded;
      })
      .filter((peer): peer is PeerNode => peer !== null);
    // Rewrite the store when a legacy token moved into a credential so the
    // secret lives at exactly one place from here on.
    if (migrated) persistPeers(peers);
    return peers;
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

export const nodesStore = new Store<NodesState>({
  self: null,
  selfStatus: "unknown",
  peers: loadPeers(),
});

/**
 * A 401 means the node answered — that's an auth problem for the TokenGate,
 * not an outage, so auth rejections stay fatal while other failures degrade
 * to "offline". Matched by name so tests can stub the api module without
 * the `AuthError` class.
 */
const isAuthFailure = (error: unknown): boolean =>
  error instanceof Error && error.name === "AuthError";

/** Move the local-node reachability marker — no-ops when already there. */
const setSelfStatus = (selfStatus: SelfStatus): void => {
  if (nodesStore.state.selfStatus === selfStatus) return;
  nodesStore.setState((prev) => ({ ...prev, selfStatus }));
};

/** Registered peers, sorted by name for stable display. */
export const getPeers = (): ReadonlyArray<PeerNode> => nodesStore.state.peers;

/** A disabled peer is parked, not removed — absent `enabled` reads as on. */
export const isPeerEnabled = (peer: PeerNode): boolean => peer.enabled !== false;

/**
 * The local node's park switch — a client pref (`settingsStore`), not node
 * state, so it survives without any server round-trip. `false` stops this
 * machine's sessions/projects/agents merging into the federated lists and
 * closes its event feed — the same skip a disabled peer gets — but its API
 * stays reachable: the origin is the transport every call lands on, so
 * `nodeTarget`/`localTarget` are unaffected.
 */
export const isLocalNodeEnabled = (): boolean => settingsStore.state.localNodeEnabled;

/** Set/clear the local node's parked state — re-enabling restores fan-out on the next refetch. */
export const setLocalNodeEnabled = (enabled: boolean): void => {
  setSettings({ localNodeEnabled: enabled });
};

/**
 * Whether any enabled peer is registered — gates every federation UI
 * affordance. The local node's own enable flag doesn't count: multi-node
 * is about whether rows need a node tag and peers need probing/feed
 * subscriptions, not about whether this machine contributes.
 */
export const isMultiNode = (): boolean => nodesStore.state.peers.some(isPeerEnabled);

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

/** url → the `{scheme, host, port}` triple the managed registry stores. */
export const peerUrlParts = (
  baseUrl: string,
): { scheme: "http" | "https"; host: string; port: number } => {
  const url = new URL(baseUrl);
  const scheme = url.protocol === "https:" ? "https" : "http";
  const port = url.port === "" ? (scheme === "https" ? 443 : 80) : Number(url.port);
  return { scheme, host: url.hostname, port };
};

/**
 * The port a node address with no `:port` lands on — `sepia serve`'s default.
 * Peers are sepia nodes first, so a bare `http` address means :8787, not :80;
 * https keeps the web default (443) since TLS front ends sit on it.
 */
export const NODE_DEFAULT_PORTS = { http: 8787, https: 443 } as const;

/** A successfully parsed node address — the canonical origin plus its parts. */
export interface ParsedNodeAddress {
  /** Canonical origin (no trailing slash, default ports elided). */
  readonly url: string;
  readonly scheme: "http" | "https";
  readonly host: string;
  readonly port: number;
}

export type NodeAddressParse =
  | { readonly ok: true; readonly address: ParsedNodeAddress }
  | { readonly ok: false; readonly error: string };

/**
 * The unified node-address field's parser — a bare `host`, a `host:port`
 * pair, or a full `http(s)://…` address (a pasted URL's path/userinfo is
 * dropped; the registry stores origins). An address with no `:port` lands on
 * the scheme's sepia default (NODE_DEFAULT_PORTS), so `thinkpad`,
 * `http://thinkpad` and `thinkpad:8787` all mean the same node. Pure —
 * returns the parts or a field-validator message, never throws.
 */
export const parseNodeAddress = (input: string): NodeAddressParse => {
  const trimmed = input.trim();
  if (trimmed === "") return { ok: false, error: "Address is required" };
  if (/\s/.test(trimmed)) {
    return { ok: false, error: "Enter a hostname or an http(s) address" };
  }
  const hasScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed);
  if (hasScheme && !/^https?:\/\//i.test(trimmed)) {
    return { ok: false, error: "Only http:// and https:// addresses are supported" };
  }
  // The authority substring — scheme, userinfo and path/query peeled off —
  // so an explicit `:port` survives detection even when it's a URL-default
  // port (`new URL("http://h:80").port` elides to "", same as no port).
  const authority = trimmed
    .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
    .split(/[/?#]/, 1)[0]
    ?.split("@")
    .pop();
  if (authority === undefined || authority === "") {
    return { ok: false, error: "Host is required" };
  }
  const explicitPort = /:(\d+)$/.exec(authority)?.[1];
  let url: URL;
  try {
    // `sepia://` stands in for "no scheme given" — non-special schemes still
    // split authority/port but don't imply a protocol of their own.
    url = new URL(hasScheme ? trimmed : `sepia://${trimmed}`);
  } catch {
    return { ok: false, error: "Enter a hostname or an http(s) address" };
  }
  if (url.hostname === "") return { ok: false, error: "Host is required" };
  const scheme = url.protocol === "https:" ? ("https" as const) : ("http" as const);
  const port =
    url.port !== ""
      ? Number(url.port)
      : explicitPort !== undefined
        ? Number(explicitPort)
        : NODE_DEFAULT_PORTS[scheme];
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return { ok: false, error: "Port must be a number from 1 to 65535" };
  }
  // Rebuild through a special-scheme URL so the origin canonicalizes —
  // default ports elide (`https://h` stays port-less, `http://h:80` too).
  const canonical = new URL(`${scheme}://${url.hostname}`);
  canonical.port = String(port);
  return { ok: true, address: { url: canonical.origin, scheme, host: url.hostname, port } };
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

/**
 * Set/clear a peer's parked state. Disabled peers stay in the registry (and
 * in Settings → Nodes) but are skipped everywhere the app fans out — see
 * `PeerNode.enabled`. Re-enabling restores normal fan-out on the next refetch.
 */
export const setPeerEnabled = (id: string, enabled: boolean): void => {
  commitPeers(
    nodesStore.state.peers.map((peer) =>
      peer.id !== id ? peer : { ...peer, enabled: enabled ? undefined : false },
    ),
  );
};

/** Set/clear a peer nickname — an empty/whitespace alias reverts to `name`. */
export const setPeerAlias = (id: string, alias: string): void => {
  const trimmed = alias.trim();
  commitPeers(
    nodesStore.state.peers.map((peer) =>
      peer.id !== id
        ? peer
        : trimmed === ""
          ? { ...peer, alias: undefined }
          : { ...peer, alias: trimmed },
    ),
  );
};

const commitPeers = (peers: ReadonlyArray<PeerNode>): void => {
  persistPeers(peers);
  nodesStore.setState((prev) => ({ ...prev, peers }));
};

/**
 * What a node form submits for a direct peer's credential — a link to a
 * stored credential (`credentialId`), or a fresh secret to file under the
 * peer's name in the credential store.
 */
export type PeerCredentialSpec =
  | { readonly credentialId: string }
  | { readonly secret: string; readonly label?: string };

/**
 * The secret a spec resolves to — a stored credential's secret, the typed
 * secret, or null. A dangling `credentialId` (deleted between render and
 * submit) resolves to null so the peer registers credential-less rather
 * than pointing at nothing.
 */
const specSecret = (spec: PeerCredentialSpec | null): string | null => {
  if (spec === null) return null;
  if ("credentialId" in spec) return credentialById(spec.credentialId)?.secret ?? null;
  const secret = spec.secret.trim();
  return secret === "" ? null : secret;
};

/**
 * File the peer's credential in the store when the spec carried a fresh
 * secret; a stored-credential spec just relinks. Returns the credentialId
 * to persist on the peer, or undefined for no auth.
 */
const linkCredential = (
  spec: PeerCredentialSpec | null,
  secret: string | null,
  fallbackLabel: string,
): string | undefined => {
  if (secret === null) return undefined;
  if (spec !== null && "credentialId" in spec) return spec.credentialId;
  const label = spec !== null && "label" in spec ? spec.label : undefined;
  return addCredential({
    label: label?.trim() === "" || label === undefined ? fallbackLabel : label,
    secret,
  }).id;
};

/** Fetch + validate a peer, then register it. Throws on unreachable/401. */
export const addPeer = async (
  url: string,
  credential: PeerCredentialSpec | null,
): Promise<PeerNode> => {
  const target: ApiTarget = {
    baseUrl: normalizeNodeUrl(url),
    token: specSecret(credential),
    timeoutMs: PROBE_TIMEOUT_MS,
  };
  const descriptor = await getNode(target);
  const self = nodesStore.state.self;
  if (self !== null && descriptor.id === self.id) {
    throw new Error("That's this machine — it's already in the list");
  }
  const credentialId = linkCredential(credential, target.token, descriptor.name);
  const peer: PeerNode = {
    id: descriptor.id,
    name: descriptor.name,
    url: target.baseUrl,
    ...(credentialId !== undefined ? { credentialId } : {}),
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
  return addPeer(baseUrl, { secret: token });
};

/** The secret a peer's credential link resolves to — null when unlinked or dangling. */
export const peerSecret = (peer: PeerNode): string | null =>
  credentialById(peer.credentialId)?.secret ?? null;

/**
 * What the Settings → Nodes edit form submits: an address change and/or a
 * credential change and/or a routing change. `via` absent keeps the current
 * routing.
 */
export interface PeerEntryUpdate {
  readonly url?: string;
  /**
   * Direct-peer credential — absent keeps the current link; `null` clears
   * it; `{credentialId}` links a stored credential; `{secret, label?}`
   * files a new one in the credential store and links it. Only applies
   * when the resulting routing is direct.
   */
  readonly credential?: PeerCredentialSpec | null;
  /**
   * Gateway credential — the raw secret submitted to this node's managed
   * registry. `SECRET_MASK` echoes the form's masked field and keeps the
   * stored credential; "" clears it; anything else replaces it. Only
   * applies when the resulting routing is gateway.
   */
  readonly token?: string;
  /**
   * Routing target — absent keeps the current mode. Switching to "gateway"
   * hands the credential to this node's managed registry (the entry is
   * created when missing); switching to "direct" drops the managed entry
   * and the browser resumes holding the credential.
   */
  readonly via?: "direct" | "gateway";
}

/**
 * Apply an edit-form save to a peer. A direct peer's url/credentialId update
 * in the browser registry; a `via: "gateway"` peer's credential lives in the
 * managed registry, so url + auth changes go through `updateServer` — the
 * existing entry is fetched first so untouched fields (label, SSH config, a
 * masked secret) round-trip instead of being clobbered by the PATCH's
 * full-replace semantics.
 *
 * A routing change moves the credential with it. Direct → gateway creates
 * (or repairs) the managed entry carrying the resolved credential secret,
 * then unlinks the browser's copy (the credential itself stays in the
 * store — other peers may share it). Gateway → direct deletes the managed
 * entry — the stored secret can't come back to the browser (the registry
 * only ever returns masks), so the submitted credential spec is the whole
 * credential.
 */
export const updatePeerEntry = async (id: string, update: PeerEntryUpdate): Promise<void> => {
  const peer = nodesStore.state.peers.find((p) => p.id === id);
  if (peer === undefined) return;
  const url = update.url === undefined ? peer.url : normalizeNodeUrl(update.url);
  const keepToken = update.token === undefined || update.token === SECRET_MASK;
  const secret = keepToken ? null : update.token?.trim() || null;
  const { scheme, host, port } = peerUrlParts(url);
  const toGateway =
    update.via === "gateway" || (update.via === undefined && peer.via === "gateway");

  if (toGateway) {
    const entry =
      peer.serverId === undefined
        ? undefined
        : (await listServers()).find((s) => s.id === peer.serverId);
    if (entry === undefined) {
      // Direct → gateway, or a gateway peer whose managed entry vanished
      // (removed via Settings → Servers): (re)create the credential entry.
      // A kept token submits the peer's resolved credential — on a
      // transition it moves to the node's store; a cleared one means no
      // auth upstream.
      const kept = peerSecret(peer);
      const created = await createServer({
        label: peer.alias ?? peer.name,
        host,
        port,
        scheme,
        auth: keepToken
          ? kept === null
            ? null
            : { type: "token", secret: kept }
          : secret === null
            ? null
            : { type: "token", secret },
        ssh: null,
      });
      commitPeers(
        nodesStore.state.peers.map((p) =>
          p.id !== id
            ? p
            : { ...p, url, credentialId: undefined, via: "gateway", serverId: created.id },
        ),
      );
      return;
    }
    await updateServer(entry.id, {
      label: entry.label,
      host,
      port,
      scheme,
      auth: keepToken
        ? entry.auth === null
          ? null
          : { type: entry.auth.type, user: entry.auth.user, secret: SECRET_MASK }
        : secret === null
          ? null
          : { type: "token", secret },
      ssh:
        entry.ssh === null
          ? null
          : {
              host: entry.ssh.host,
              port: entry.ssh.port,
              user: entry.ssh.user,
              // A masked key keeps the stored material; a key path
              // round-trips as itself.
              key: entry.ssh.key,
            },
    });
    commitPeers(
      nodesStore.state.peers.map((p) =>
        p.id !== id
          ? p
          : { ...p, url, credentialId: undefined, via: "gateway", serverId: entry.id },
      ),
    );
    return;
  }

  // Direct target. Leaving gateway drops the managed entry (best-effort —
  // an unreachable node must not strand the switch) since the stored
  // credential can't return to the browser — the submitted credential spec
  // (or none) is the whole link from here on.
  const leavingGateway = peer.via === "gateway";
  if (leavingGateway && peer.serverId !== undefined) {
    await deleteServer(peer.serverId).catch(() => undefined);
  }
  const spec = update.credential;
  const credentialId =
    spec === undefined
      ? leavingGateway
        ? undefined
        : peer.credentialId
      : linkCredential(spec, specSecret(spec), peer.alias ?? peer.name);
  commitPeers(
    nodesStore.state.peers.map((p) =>
      p.id !== id
        ? p
        : {
            ...p,
            url,
            credentialId,
            ...(leavingGateway ? { via: undefined, serverId: undefined } : {}),
          },
    ),
  );
};

// --- Gateway-mode peers (docs/protocol.md phase 3) ---------------------------
//
// A `via: "gateway"` peer keeps no credential in the browser: the add flow
// registers the peer in this node's managed-server registry (`/api/servers`,
// encrypted at rest) and every call rides `ANY /api/gateway/<serverId>`,
// where the server injects the stored credential. Probing goes *through* the
// gateway — a peer the browser can't reach directly is exactly the case
// gateway mode exists for, so direct probes would always fail.

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
  const { scheme, host, port } = peerUrlParts(baseUrl);
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
  const { scheme, host, port } = peerUrlParts(baseUrl);
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

/**
 * Populate `nodesStore.self` from the local node's own /api/node. A failed
 * probe clears `self` and marks the node offline — the error still
 * propagates so the query sees it — except a 401, which is reachable (an
 * auth problem, not an outage) and left for the TokenGate.
 */
export const refreshSelf = async (): Promise<NodeDescriptor> => {
  try {
    const descriptor = await getNode(localTarget());
    setLocalNodeAlias(descriptor.id);
    nodesStore.setState((prev) => ({ ...prev, self: descriptor, selfStatus: "online" }));
    return descriptor;
  } catch (error) {
    if (!isAuthFailure(error)) {
      nodesStore.setState((prev) => ({ ...prev, self: null, selfStatus: "offline" }));
    }
    throw error;
  }
};

/**
 * API target for a registered peer. A `via: "gateway"` peer resolves to this
 * node's `/api/gateway/<serverId>` forward — every call site (fan-out lists,
 * session actions, SSE streams) routes through it unchanged; a direct peer
 * resolves to its own origin + the linked credential's secret.
 */
export const peerTarget = (peer: PeerNode): ApiTarget => {
  // A disabled peer resolves to the same deliberately-unreachable sentinel
  // `nodeTarget` gives unknown ids — any call site that slipped past the
  // enabled-filters fails safely instead of touching the local machine.
  if (!isPeerEnabled(peer)) {
    return { baseUrl: `http://${peer.id}.invalid`, token: null, timeoutMs: PEER_TIMEOUT_MS };
  }
  if (peer.via === "gateway" && peer.serverId !== undefined) {
    return gatewayTarget(peer.serverId);
  }
  return { baseUrl: peer.url, token: peerSecret(peer), timeoutMs: PEER_TIMEOUT_MS };
};

/**
 * Resolve a row's `node` field to the API target that owns it. Local rows
 * (undefined/"local") hit the same-origin server — a disabled local still
 * resolves here since the origin is the transport, not just a data source;
 * only its merge contribution is parked. An unknown peer id — or a peer the
 * user disabled — yields a deliberately unreachable target so stale rows
 * fail instead of silently mutating the local machine.
 */
export const nodeTarget = (node: string | undefined): ApiTarget => {
  if (node === undefined || node === LOCAL_NODE_ID) return localTarget();
  const peer = nodesStore.state.peers.find((p) => p.id === node);
  if (peer !== undefined) return peerTarget(peer);
  return { baseUrl: `http://${node}.invalid`, token: null, timeoutMs: PEER_TIMEOUT_MS };
};

/**
 * Whether the browser is on the machine hosting the UI — loopback origin
 * means "this machine" labels are honest; a LAN/remote origin (a phone on
 * the network) is a different machine and must not claim it.
 */
export const isLocalAccess = (): boolean =>
  typeof location !== "undefined" &&
  (location.hostname === "localhost" ||
    location.hostname === "127.0.0.1" ||
    location.hostname === "[::1]");

/** Display name for a node id — nicknames win, then self-reported names. */
export const nodeName = (node: string | undefined): string => {
  if (node === undefined || node === LOCAL_NODE_ID) {
    return (
      settingsStore.state.localNodeName ??
      nodesStore.state.self?.name ??
      (isLocalAccess() ? "this machine" : "local")
    );
  }
  const peer = nodesStore.state.peers.find((p) => p.id === node);
  return peer === undefined ? node : (peer.alias ?? peer.name);
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
    return settingsStore.state.localNodeName ?? self?.name ?? "local";
  }
  const peer = nodesStore.state.peers.find((p) => p.id === node);
  if (peer !== undefined) return peer.alias ?? peer.name;
  if (node === "") return "local";
  return node.length > 14 ? `${node.slice(0, 14)}…` : node;
};

// --- Fan-out fetches ---------------------------------------------------------
//
// Each merged list calls every registered node. Every leg degrades the same
// way — a node that doesn't answer contributes nothing — and a failed LOCAL
// leg additionally flips `selfStatus` to "offline" so the UI can mark this
// machine down (docs/protocol.md failure model). The one exception: a local
// 401 stays fatal — the machine answered, so it's an auth problem for the
// TokenGate, not an outage.

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

/**
 * Fold a fan-out's settled legs into rows. `local` is the local node's leg —
 * undefined when this machine is parked (its leg never ran, so `selfStatus`
 * stays with whatever `refreshSelf` last saw); `peers` are the enabled
 * peers' legs in registry order. A rejected leg contributes nothing; the
 * local leg additionally drives `selfStatus` (success → online, failure →
 * offline). A local 401 still throws so the caller's AuthError → TokenGate
 * path keeps working.
 */
const mergeFanOut = <T>(
  local: PromiseSettledResult<T[]> | undefined,
  peers: ReadonlyArray<PromiseSettledResult<T[]>>,
): T[] => {
  const rows: T[] = [];
  if (local !== undefined) {
    if (local.status === "fulfilled") {
      setSelfStatus("online");
      rows.push(...local.value);
    } else {
      if (isAuthFailure(local.reason)) throw local.reason;
      setSelfStatus("offline");
    }
  }
  for (const leg of peers) {
    if (leg.status === "fulfilled") rows.push(...leg.value);
  }
  return rows;
};

// `withLocks` rides every fan-out leg so rows can show held state. Each leg
// costs the node one `session/list` probe — a throwaway agent spawn per
// agent with no live connection — but the server's lockCache TTL
// (SEPIA_LOCK_TTL_MS, ~5s) caps that at one probe round per TTL window, and
// list refetches are event-driven (window focus, mutation invalidations),
// not polled. Nodes too old to know the flag ignore it and report
// `locked: false` rows.
export const listAllSessions = async (): Promise<SessionSummary[]> => {
  // Disabled nodes contribute nothing — same as if they weren't registered.
  // The local leg skips too when this machine is parked: its API stays
  // reachable as the client's origin (the transport), only the merge stops.
  const local = isLocalNodeEnabled();
  const peers = nodesStore.state.peers.filter(isPeerEnabled);
  const multi = peers.length > 0;
  const settled = await Promise.allSettled([
    ...(local
      ? [
          listSessions(localTarget(), { withLocks: true }).then((list) =>
            list.map((s) => tagSession(s, multi ? LOCAL_NODE_ID : undefined)),
          ),
        ]
      : []),
    ...peers.map((peer) =>
      listSessions(peerTarget(peer), { withLocks: true }).then((list) =>
        list.map((s) => tagSession(s, peer.id)),
      ),
    ),
  ]);
  return mergeFanOut(local ? settled[0] : undefined, settled.slice(local ? 1 : 0));
};

export const listAllProjects = async (): Promise<Project[]> => {
  const local = isLocalNodeEnabled();
  const peers = nodesStore.state.peers.filter(isPeerEnabled);
  const multi = peers.length > 0;
  const settled = await Promise.allSettled([
    ...(local
      ? [
          listProjects(localTarget()).then((data) =>
            data.projects.map((p) => tagProject(p, multi ? LOCAL_NODE_ID : undefined)),
          ),
        ]
      : []),
    ...peers.map((peer) =>
      listProjects(peerTarget(peer)).then((data) =>
        data.projects.map((p) => tagProject(p, peer.id)),
      ),
    ),
  ]);
  return mergeFanOut(local ? settled[0] : undefined, settled.slice(local ? 1 : 0));
};

/** Union of agent rosters across nodes — deduped by agent id, local wins. */
export const listAllAgents = async (): Promise<AgentInfo[]> => {
  const local = isLocalNodeEnabled();
  const peers = nodesStore.state.peers.filter(isPeerEnabled);
  const settled = await Promise.allSettled([
    ...(local ? [listAgents(localTarget())] : []),
    ...peers.map((peer) => listAgents(peerTarget(peer))),
  ]);
  // Rows merge local-first, so first-wins dedupe keeps this machine's
  // roster authoritative whenever the local leg answered.
  const merged = new Map<string, AgentInfo>();
  for (const agent of mergeFanOut(local ? settled[0] : undefined, settled.slice(local ? 1 : 0))) {
    if (!merged.has(agent.id)) merged.set(agent.id, agent);
  }
  return [...merged.values()];
};
