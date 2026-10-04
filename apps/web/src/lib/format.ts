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
