import { LOCAL_NODE_ID, nodeKey, projectName, sessionKey } from "./format";
import type { SessionSummary } from "./types";

/**
 * Row data for the Folders tree: directory nodes built from session cwds,
 * session leaves, and — in multi-node mode — a node group per machine.
 */
export type SessionTreeData =
  | {
      readonly kind: "dir";
      readonly label: string;
      readonly cwd: string;
      readonly count: number;
      /**
       * Owning node's normalized key (`"local"` for this machine). Set only
       * when the tree groups by node; folder actions route through it.
       */
      readonly node?: string;
    }
  | { readonly kind: "session"; readonly session: SessionSummary }
  | {
      readonly kind: "node";
      /** Normalized node key — `"local"` for this machine, the peer id else. */
      readonly node: string;
      readonly count: number;
    };

export const SESSION_TREE_ROOT = "root";

export interface SessionTreeIndex {
  readonly dataMap: Map<string, SessionTreeData>;
  readonly childrenMap: Map<string, string[]>;
  readonly rootChildren: string[];
  /**
   * Every expandable non-root item id — dir rows plus node groups — in
   * build order. The tree auto-expands ids it hasn't seen before.
   */
  readonly dirIds: string[];
}

/**
 * The item id of the dir row a session hangs under. Single-node mode keeps
 * the legacy `dir:<cwd>` form so persisted expansion state still applies;
 * grouped mode namespaces it `dir:<node>:<cwd>` so two machines' same-path
 * folders collapse independently.
 */
export const sessionDirId = (cwd: string, node: string | undefined, grouped: boolean): string =>
  grouped ? `dir:${nodeKey(node)}:${cwd}` : `dir:${cwd}`;

/**
 * Builds the Folders tree's item maps from session cwds — each path segment
 * is a collapsible dir node; sessions hang off the dir for their exact cwd.
 * Single-child dir chains fold GitHub-style (home/luis/GitHub → one node).
 *
 * `groupByNode` (multi-node mode) lifts the tree one level: `node:<key>`
 * groups at the root, each holding its own dir tree, so identical cwds on
 * different machines never merge. Node groups order local-first, then by
 * node key.
 */
export const treeFromSessions = (
  sessions: ReadonlyArray<SessionSummary>,
  opts: { readonly groupByNode?: boolean } = {},
): SessionTreeIndex => {
  const dataMap = new Map<string, SessionTreeData>();
  const childrenMap = new Map<string, string[]>();
  const dirIds: string[] = [];

  interface DirNode {
    label: string;
    path: string;
    children: string[];
  }

  // Builds one scope's dir tree. `idPrefix` namespaces item ids (`"<node>:"`
  // in grouped mode); `node` is stamped on dir rows for action routing.
  const buildScope = (
    scopeSessions: ReadonlyArray<SessionSummary>,
    idPrefix: string,
    node: string | undefined,
  ): string[] => {
    const dirs = new Map<string, DirNode>();
    const sessionIdsByDir = new Map<string, string[]>();

    const dirFor = (path: string): DirNode => {
      const existing = dirs.get(path);
      if (existing !== undefined) return existing;
      const dir: DirNode = {
        label: projectName(path) || "/",
        path,
        children: [],
      };
      dirs.set(path, dir);
      return dir;
    };

    for (const session of scopeSessions) {
      // node:agent:id — bare ids collide across agents and would overwrite rows.
      const id = `session:${sessionKey(session)}`;
      dataMap.set(id, { kind: "session", session });
      const segments = session.cwd.split("/").filter(Boolean);
      let prefix = "";
      let parent = dirFor("/");
      for (const segment of segments) {
        prefix += `/${segment}`;
        const dir = dirFor(prefix);
        const dirId = `dir:${idPrefix}${prefix}`;
        if (!parent.children.includes(dirId)) parent.children.push(dirId);
        parent = dir;
      }
      const leafIds = sessionIdsByDir.get(parent.path) ?? [];
      leafIds.push(id);
      sessionIdsByDir.set(parent.path, leafIds);
    }

    // Attach sessions to their leaf dir + fold single-child dir chains.
    const strip = `dir:${idPrefix}`.length;
    const walk = (path: string): string => {
      // Fold chains of dirs with exactly one dir child into "a/b/c" labels,
      // keeping the deepest path as the id so dir:<cwd> lookups still work.
      let merged = dirs.get(path)!;
      let labels = merged.label;
      while (merged.children.length === 1 && sessionIdsByDir.get(merged.path) === undefined) {
        merged = dirs.get(merged.children[0]!.slice(strip))!;
        labels = `${labels}/${merged.label}`;
      }
      const kids = merged.children;
      const sessionKids = sessionIdsByDir.get(merged.path) ?? [];
      const dirId = `dir:${idPrefix}${merged.path}`;
      dataMap.set(dirId, { kind: "dir", label: labels, cwd: merged.path, count: 0, node });
      const entries: string[] = [];
      let total = sessionKids.length;
      for (const kid of [...kids, ...sessionKids]) {
        if (kid.startsWith("dir:")) {
          const compactedId = walk(kid.slice(strip));
          entries.push(compactedId);
          total += (dataMap.get(compactedId) as { count: number }).count;
        } else {
          entries.push(kid);
        }
      }
      childrenMap.set(dirId, entries);
      (dataMap.get(dirId) as { count: number }).count = total;
      dirIds.push(dirId);
      return dirId;
    };

    const rootKids = dirs.get("/")?.children ?? [];
    return rootKids.map((top) => walk(top.slice(strip)));
  };

  if (opts.groupByNode !== true) {
    const rootChildren = buildScope(sessions, "", undefined);
    childrenMap.set(SESSION_TREE_ROOT, rootChildren);
    return { dataMap, childrenMap, rootChildren, dirIds };
  }

  // Group sessions by normalized node key — `"local"` for this machine.
  const byNode = new Map<string, SessionSummary[]>();
  for (const session of sessions) {
    const key = nodeKey(session.node);
    const bucket = byNode.get(key);
    if (bucket === undefined) byNode.set(key, [session]);
    else bucket.push(session);
  }
  const order = [...byNode.keys()].sort((a, b) => {
    if (a === LOCAL_NODE_ID || b === LOCAL_NODE_ID)
      return a === b ? 0 : a === LOCAL_NODE_ID ? -1 : 1;
    return a.localeCompare(b);
  });

  const rootChildren: string[] = [];
  for (const node of order) {
    const kids = buildScope(byNode.get(node)!, `${node}:`, node);
    const id = `node:${node}`;
    const count = kids.reduce(
      (sum, kid) => sum + ((dataMap.get(kid) as { count: number } | undefined)?.count ?? 0),
      0,
    );
    dataMap.set(id, { kind: "node", node, count });
    childrenMap.set(id, kids);
    dirIds.push(id);
    rootChildren.push(id);
  }
  childrenMap.set(SESSION_TREE_ROOT, rootChildren);
  return { dataMap, childrenMap, rootChildren, dirIds };
};
