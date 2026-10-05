import type { MessageUsage } from "./types";

export function formatUpdated(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diffMinutes = Math.round((Date.now() - then) / 60000);
  if (diffMinutes < 1) return "just now";
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  const hours = Math.round(diffMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

const compactNumber = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const groupedNumber = new Intl.NumberFormat("en-US");

/** "$0.02" — "" when the store recorded no (or a non-positive) cost. */
export const formatCost = (cost: number): string => {
  if (!Number.isFinite(cost) || cost <= 0) return "";
  return `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}`;
};

/** Compact footer form — "↑ 1.2k · ↓ 340 tok", plus " · $0.02" when priced. */
export const usageLabel = (usage: MessageUsage): string => {
  const base = `↑ ${compactNumber.format(usage.input)} · ↓ ${compactNumber.format(usage.output)} tok`;
  const cost = usage.cost === undefined ? "" : formatCost(usage.cost);
  return cost === "" ? base : `${base} · ${cost}`;
};

/** Hover-title breakdown — every tier the agent recorded, grouped digits. */
export const formatUsage = (usage: MessageUsage): string => {
  const parts = [
    `${groupedNumber.format(usage.input)} input`,
    `${groupedNumber.format(usage.output)} output`,
  ];
  if (usage.cacheRead !== undefined && usage.cacheRead > 0) {
    parts.push(`${groupedNumber.format(usage.cacheRead)} cache read`);
  }
  if (usage.cacheWrite !== undefined && usage.cacheWrite > 0) {
    parts.push(`${groupedNumber.format(usage.cacheWrite)} cache write`);
  }
  if (usage.thinking !== undefined && usage.thinking > 0) {
    parts.push(`${groupedNumber.format(usage.thinking)} thinking`);
  }
  const cost = usage.cost === undefined ? "" : formatCost(usage.cost);
  if (cost !== "") parts.push(cost);
  return parts.join(" · ");
};

/** "950ms" / "1.2s" / "1m 4s" — the parenthesized suffix on a tool-call marker. */
export const formatDuration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms < 0) return "";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return seconds === 0 ? `${minutes}m` : `${minutes}m ${seconds}s`;
};

/** Ordinary turn endings — the norm, never worth a marker. */
const QUIET_FINISH_REASONS = new Set(["stop", "end_turn", "tool_calls"]);

/**
 * Finish reasons worth surfacing on an assistant row: "length", "error",
 * and anything else unusual. The quiet endings return null so the footer
 * adds no noise.
 */
export const finishReasonLabel = (reason: string | undefined | null): string | null =>
  reason === undefined || reason === null || reason === "" || QUIET_FINISH_REASONS.has(reason)
    ? null
    : reason;

export const projectName = (cwd: string): string => {
  const trimmed = cwd.replace(/\/+$/, "");
  const last = trimmed.split("/").pop();
  return last === undefined || last === "" ? cwd : last;
};

/**
 * Node segment used in `node:agent:id` keys for the machine serving this UI.
 * Rows the merged lists tag for the local node carry this sentinel — never
 * the server's real node id — so keys stay stable before `GET /api/node`
 * resolves and don't churn when the identity lands.
 */
export const LOCAL_NODE_ID = "local";

// The local node's server-issued id, learned via setLocalNodeAlias — lets a
// `node_a1b2:agent:id` key minted elsewhere still resolve to the local row.
let localNodeAlias: string | null = null;
export const setLocalNodeAlias = (id: string): void => {
  localNodeAlias = id;
};

/** A row belongs to the node serving this UI. */
export const isLocalNode = (node: string | undefined): boolean =>
  node === undefined || node === LOCAL_NODE_ID || node === localNodeAlias;

/**
 * Whether a session key names a row on this machine. `agent:id` and bare-id
 * forms are implicitly local; `node:agent:id` checks the node segment
 * (the server-issued local alias counts too). Used to tell "session gone"
 * apart from "the node is unreachable" when a selection doesn't resolve.
 */
export const keyTargetsLocalNode = (key: string | null | undefined): boolean => {
  if (key === null || key === undefined || key === "") return false;
  const parts = key.split(":");
  return parts.length < 3 || isLocalNode(parts[0]);
};

/** Canonical node segment for keys: local rows normalize to "local". */
export const nodeKey = (node: string | undefined): string =>
  node === undefined || node === localNodeAlias ? LOCAL_NODE_ID : node;

/**
 * Session key — `<node>:<agent>:<id>` once peers are registered, the legacy
 * `<agent>:<id>` in single-node mode (rows then carry no `node`). Bare ids
 * collide across agents, and `agent:id` collides across nodes.
 */
export const sessionKey = (session: {
  readonly agent: string;
  readonly id: string;
  readonly node?: string;
}): string =>
  session.node === undefined
    ? `${session.agent}:${session.id}`
    : `${nodeKey(session.node)}:${session.agent}:${session.id}`;

/** Node-scoped project key — project ids are node-local (`proj_*`). */
export const projectKey = (project: { readonly id: string; readonly node?: string }): string =>
  project.node === undefined ? project.id : `${nodeKey(project.node)}:${project.id}`;

/**
 * Strip the node prefix from a namespaced project reference so the owning
 * node's API sees its own bare id. Bare `proj_*` ids pass through untouched.
 */
export const bareProjectId = (ref: string): string => {
  const index = ref.lastIndexOf(":");
  return index === -1 ? ref : ref.slice(index + 1);
};

// --- Catalog keys -------------------------------------------------------------
// Settings → Agents/Models/Nodes address offerings across the federation as
// `node:agent` (agents) and `node:agent:model` (models) keys — the same
// nodeKey-segment convention session/project keys use, so the local alias
// and the "local" sentinel normalize identically.

/**
 * The model segment for "whatever the agent runs by default" — agents don't
 * advertise a model roster on the wire, so the agent default is itself a
 * catalog entry keyed under this sentinel id (e.g. `local:devin:auto`).
 */
export const AUTO_MODEL_ID = "auto";

/** Agent catalog key — `<nodeKey>:<agentId>` (`local:devin`, `node_x:cline`). */
export const agentKey = (node: string | undefined, agent: string): string =>
  `${nodeKey(node)}:${agent}`;

/**
 * Model catalog key — `<nodeKey>:<agentId>:<modelId>`. A null/empty model id
 * mints the agent-default entry (`AUTO_MODEL_ID`); a model id may itself
 * carry colons — keys parse by taking the first two segments and joining
 * the tail.
 */
export const catalogKey = (
  node: string | undefined,
  agent: string,
  model: string | null | undefined,
): string =>
  `${agentKey(node, agent)}:${model === null || model === undefined || model === "" ? AUTO_MODEL_ID : model}`;

/**
 * Parse a `node:agent` catalog key. The node segment canonicalizes through
 * `nodeKey` (local aliases collapse to "local"); a missing/empty segment
 * fails — `null`, not a partial key.
 */
export const parseAgentKey = (key: string): { node: string; agent: string } | null => {
  // Exactly `node:agent` — a longer catalog key must not parse as one.
  const parts = key.split(":");
  if (parts.length !== 2 || parts[0] === "" || parts[1] === "") return null;
  return { node: nodeKey(parts[0]), agent: parts[1] };
};

/**
 * Parse a `node:agent:model` catalog key — two segments minimum for
 * node+agent; everything after the second colon is the model id (model ids
 * may carry colons). Malformed keys return `null`.
 */
export const parseCatalogKey = (
  key: string,
): { node: string; agent: string; model: string } | null => {
  const parts = key.split(":");
  if (parts.length < 3) return null;
  const [node, agent, ...rest] = parts;
  const model = rest.join(":");
  if (node === undefined || agent === undefined || model === "") return null;
  return { node: nodeKey(node), agent, model };
};

/**
 * Semantic equality for `node:agent:model` keys — node segments compare
 * through `nodeKey` so a local-alias form matches the "local" sentinel.
 */
export const sameCatalogKey = (
  a: string | null | undefined,
  b: string | null | undefined,
): boolean => {
  if (a === b) return true;
  if (a === null || a === undefined || b === null || b === undefined) return false;
  const ka = parseCatalogKey(a);
  const kb = parseCatalogKey(b);
  if (ka === null || kb === null) return false;
  return ka.node === kb.node && ka.agent === kb.agent && ka.model === kb.model;
};

/**
 * Semantic equality for session keys. Several literal forms address one
 * row — `node:agent:id`, the local-implicit `agent:id`, and the
 * server-issued local node id once the alias lands — so consumers that
 * compare keys (e.g. the URL↔store sync) must compare by segments, not
 * string identity, or equivalent forms ping-pong. Bare ids stay
 * agent-ambiguous: they only match another bare id with the same string.
 */
export const sameSessionKey = (
  a: string | null | undefined,
  b: string | null | undefined,
): boolean => {
  if (a === b) return true;
  if (a === null || a === undefined || b === null || b === undefined) return false;
  if (a === "" || b === "") return false;
  const parse = (key: string): { node: string; agent: string | null; id: string } => {
    const parts = key.split(":");
    if (parts.length >= 3) {
      // join the tail — a session id itself could carry a colon.
      return { node: nodeKey(parts[0]), agent: parts[1], id: parts.slice(2).join(":") };
    }
    if (parts.length === 2) {
      // Legacy agent:id — the node segment is implicit local.
      return { node: LOCAL_NODE_ID, agent: parts[0], id: parts[1] };
    }
    return { node: LOCAL_NODE_ID, agent: null, id: key };
  };
  const ka = parse(a);
  const kb = parse(b);
  return ka.node === kb.node && ka.agent === kb.agent && ka.id === kb.id;
};

/**
 * The URL→store selection rule: a present, non-empty `?session=` pulls the
 * store unless it names the same row the user already picked. Its absence
 * never clears a selection — a URL transition that briefly drops the param
 * used to null `selectedId`, and the auto-select effect then teleported
 * the highlight to the first row.
 */
export const shouldSyncUrlSelection = (
  searchSession: string | undefined,
  selectedId: string | null,
): boolean =>
  searchSession !== undefined && searchSession !== "" && !sameSessionKey(searchSession, selectedId);

/**
 * Finds a session by its scoped key:
 * - `node:agent:id` — exact node + agent + id (local aliases resolve to the
 *   local row),
 * - `agent:id` — legacy 2-segment key, always means the local node,
 * - `id` — bare id (old links); local rows win, then any node.
 */
export const resolveSession = <
  T extends { readonly agent: string; readonly id: string; readonly node?: string },
>(
  sessions: ReadonlyArray<T>,
  key: string | null | undefined,
): T | undefined => {
  if (key === null || key === undefined || key === "") return undefined;
  const parts = key.split(":");
  if (parts.length === 3) {
    const [node, agent, id] = parts;
    return sessions.find(
      (s) => nodeKey(s.node) === nodeKey(node) && s.agent === agent && s.id === id,
    );
  }
  if (parts.length === 2) {
    const [agent, id] = parts;
    return sessions.find((s) => isLocalNode(s.node) && s.agent === agent && s.id === id);
  }
  return (
    sessions.find((s) => isLocalNode(s.node) && s.id === key) ?? sessions.find((s) => s.id === key)
  );
};

type SessionIdentity = {
  readonly agent: string;
  readonly id: string;
  readonly node?: string;
};

type SubAgentRow = SessionIdentity & { readonly parentSessionId?: string };

/**
 * Whether `child` was spawned by `parent`. Adapters record the parent's
 * bare session id (cline embeds it in the child's own id, devin reads
 * `subagent_heads.session_id`) and both rows live in the same agent store
 * on one node — so a bare ref only counts on the full id + agent + node
 * triple; a same-id row in another store isn't the parent. Keyed refs
 * (`node:agent:id`, `agent:id`) resolve and compare semantically, which
 * keeps the local-node alias and both segment forms working.
 */
export const isSubAgentOf = <T extends SubAgentRow>(
  sessions: ReadonlyArray<T>,
  child: SubAgentRow,
  parent: SessionIdentity,
): boolean => {
  const ref = child.parentSessionId;
  if (ref === undefined || ref === "") return false;
  if (!ref.includes(":")) {
    return (
      ref === parent.id &&
      child.agent === parent.agent &&
      nodeKey(child.node) === nodeKey(parent.node)
    );
  }
  const resolved = resolveSession(sessions, ref);
  return resolved !== undefined && sameSessionKey(sessionKey(resolved), sessionKey(parent));
};

/** Sessions spawned by `parent` — its sub-agent children, list order kept. */
export const subAgentsOf = <T extends SubAgentRow>(
  sessions: ReadonlyArray<T>,
  parent: SessionIdentity | undefined,
): T[] => {
  if (parent === undefined) return [];
  const key = sessionKey(parent);
  // The self-key guard drops a pathological row that names itself parent.
  return sessions.filter(
    (s) => !sameSessionKey(sessionKey(s), key) && isSubAgentOf(sessions, s, parent),
  );
};

/**
 * Finds a session row the way mutations need it — by the row's own fields,
 * not a key string. `node`/`agent` are the owning node and agent store.
 */
export const findSessionRow = <
  T extends { readonly agent: string; readonly id: string; readonly node?: string },
>(
  sessions: ReadonlyArray<T>,
  match: { readonly id: string; readonly agent?: string; readonly node?: string },
): T | undefined =>
  sessions.find(
    (s) =>
      s.id === match.id &&
      (match.agent === undefined || s.agent === match.agent) &&
      nodeKey(s.node) === nodeKey(match.node),
  );
