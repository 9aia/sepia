import { describe, expect, it } from "vite-plus/test";
import {
  SESSION_TREE_ROOT,
  sessionDirId,
  treeFromSessions,
  type SessionTreeData,
} from "../lib/sessionTree";
import type { SessionSummary } from "../lib/types";

const session = (
  id: string,
  cwd: string,
  node?: string,
  agent: SessionSummary["agent"] = "devin",
): SessionSummary => ({
  id,
  title: id,
  cwd,
  agent,
  node,
  updatedAt: "2024-01-01T00:00:00.000Z",
  locked: false,
  lockHolderPid: null,
  source: "test",
  busy: false,
  pinned: false,
  archived: false,
  projectIds: [],
  model: null,
  spans: [],
});

const dir = (data: SessionTreeData | undefined) => {
  expect(data?.kind).toBe("dir");
  return data as Extract<SessionTreeData, { kind: "dir" }>;
};

describe("treeFromSessions — single-node (flat)", () => {
  it("builds a cwd tree with folded single-child chains and legacy dir ids", () => {
    const { dataMap, childrenMap, rootChildren, dirIds } = treeFromSessions([
      session("a", "/home/luis/GitHub/sepia"),
      session("b", "/home/luis/GitHub/sepia"),
      session("c", "/tmp"),
    ]);

    // /home → /luis → /GitHub folds into one dir row at the deepest path.
    expect(rootChildren).toEqual(["dir:/home/luis/GitHub/sepia", "dir:/tmp"]);
    const folded = dir(dataMap.get("dir:/home/luis/GitHub/sepia"));
    expect(folded.label).toBe("home/luis/GitHub/sepia");
    expect(folded.cwd).toBe("/home/luis/GitHub/sepia");
    expect(folded.count).toBe(2);
    // Flat mode stamps no node on dirs and mints no node rows.
    expect(folded.node).toBeUndefined();
    expect([...dataMap.keys()].some((id) => id.startsWith("node:"))).toBe(false);
    expect(dirIds).toEqual(["dir:/home/luis/GitHub/sepia", "dir:/tmp"]);

    expect(childrenMap.get("dir:/home/luis/GitHub/sepia")).toEqual([
      "session:devin:a",
      "session:devin:b",
    ]);
    expect(childrenMap.get("dir:/tmp")).toEqual(["session:devin:c"]);
    expect(childrenMap.get(SESSION_TREE_ROOT)).toEqual(rootChildren);
  });

  it("keeps sibling dirs unfolded under their common parent", () => {
    const { childrenMap, rootChildren } = treeFromSessions([
      session("a", "/work/one"),
      session("b", "/work/two"),
    ]);
    expect(rootChildren).toEqual(["dir:/work"]);
    expect(childrenMap.get("dir:/work")).toEqual(["dir:/work/one", "dir:/work/two"]);
  });
});

describe("treeFromSessions — multi-node (grouped)", () => {
  it("groups cwd trees under node:<key> rows, local first", () => {
    const { dataMap, childrenMap, rootChildren, dirIds } = treeFromSessions(
      [
        session("l1", "/home/luis/app", "local"),
        session("p1", "/home/luis/app", "node_p"),
        session("p2", "/srv/data", "node_p"),
      ],
      { groupByNode: true },
    );

    // Local node group leads; peers follow sorted by node key.
    expect(rootChildren).toEqual(["node:local", "node:node_p"]);
    expect(childrenMap.get(SESSION_TREE_ROOT)).toEqual(rootChildren);

    const local = dataMap.get("node:local");
    expect(local?.kind).toBe("node");
    expect((local as { count: number }).count).toBe(1);
    expect((dataMap.get("node:node_p") as { count: number }).count).toBe(2);

    // Identical cwds on two machines are separate, node-namespaced dirs.
    expect(childrenMap.get("node:local")).toEqual(["dir:local:/home/luis/app"]);
    const peerKids = childrenMap.get("node:node_p") ?? [];
    expect(peerKids).toContain("dir:node_p:/home/luis/app");
    expect(peerKids).toContain("dir:node_p:/srv/data");

    const localDir = dir(dataMap.get("dir:local:/home/luis/app"));
    const peerDir = dir(dataMap.get("dir:node_p:/home/luis/app"));
    expect(localDir.node).toBe("local");
    expect(peerDir.node).toBe("node_p");
    expect(childrenMap.get("dir:local:/home/luis/app")).toEqual(["session:local:devin:l1"]);
    expect(childrenMap.get("dir:node_p:/home/luis/app")).toEqual(["session:node_p:devin:p1"]);

    // Node groups are expandable too — the tree auto-expands unseen ids.
    expect(dirIds).toContain("node:local");
    expect(dirIds).toContain("node:node_p");
  });

  it("folds chains per node and buckets node-less rows under local", () => {
    const { dataMap, childrenMap, rootChildren } = treeFromSessions(
      [session("p1", "/a/b/c", "node_p"), session("untagged", "/legacy/dir")],
      { groupByNode: true },
    );
    expect(rootChildren).toEqual(["node:local", "node:node_p"]);
    expect(childrenMap.get("node:node_p")).toEqual(["dir:node_p:/a/b/c"]);
    expect(dir(dataMap.get("dir:node_p:/a/b/c")).label).toBe("a/b/c");
    // A row without a node still lands in this machine's group.
    expect(childrenMap.get("node:local")).toEqual(["dir:local:/legacy/dir"]);
    expect(childrenMap.get("dir:local:/legacy/dir")).toEqual(["session:devin:untagged"]);
  });

  it("orders peers by node key after local", () => {
    const { rootChildren } = treeFromSessions(
      [session("z", "/w", "node_z"), session("a", "/w", "node_a"), session("l", "/w", "local")],
      { groupByNode: true },
    );
    expect(rootChildren).toEqual(["node:local", "node:node_a", "node:node_z"]);
  });
});

describe("sessionDirId", () => {
  it("keeps the legacy form flat and namespaces when grouped", () => {
    expect(sessionDirId("/w", undefined, false)).toBe("dir:/w");
    expect(sessionDirId("/w", "local", true)).toBe("dir:local:/w");
    expect(sessionDirId("/w", "node_p", true)).toBe("dir:node_p:/w");
    // Untagged rows resolve to the local segment in grouped mode.
    expect(sessionDirId("/w", undefined, true)).toBe("dir:local:/w");
  });
});
