import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import {
  bareProjectId,
  findSessionRow,
  formatUpdated,
  isLocalNode,
  LOCAL_NODE_ID,
  nodeKey,
  projectKey,
  projectName,
  resolveSession,
  sessionKey,
  setLocalNodeAlias,
} from "../lib/format";

afterEach(() => {
  vi.useRealTimers();
});

describe("formatUpdated", () => {
  it("returns an empty string for unparseable input", () => {
    expect(formatUpdated("not a date")).toBe("");
    expect(formatUpdated("")).toBe("");
  });

  it("reports fresh timestamps as just now", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-01T12:00:00Z"));
    expect(formatUpdated("2025-06-01T11:59:40Z")).toBe("just now");
  });

  it("formats minutes, hours, and days", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-01T12:00:00Z"));
    expect(formatUpdated("2025-06-01T11:45:00Z")).toBe("15m ago");
    expect(formatUpdated("2025-06-01T09:00:00Z")).toBe("3h ago");
    expect(formatUpdated("2025-05-29T12:00:00Z")).toBe("3d ago");
  });
});

describe("projectName", () => {
  it("takes the last path segment", () => {
    expect(projectName("/home/dev/api-server")).toBe("api-server");
    expect(projectName("/")).toBe("/");
  });

  it("ignores trailing slashes", () => {
    expect(projectName("/home/dev/api-server/")).toBe("api-server");
    expect(projectName("/home/dev/api-server///")).toBe("api-server");
  });
});

describe("node key helpers", () => {
  it("treats undefined and the sentinel as local", () => {
    expect(isLocalNode(undefined)).toBe(true);
    expect(isLocalNode(LOCAL_NODE_ID)).toBe(true);
    expect(isLocalNode("node_remote")).toBe(false);
  });

  it("normalizes local rows to the sentinel in keys", () => {
    expect(nodeKey(undefined)).toBe("local");
    expect(nodeKey("node_remote")).toBe("node_remote");
  });

  it("sessionKey is agent:id without a node and node:agent:id with one", () => {
    expect(sessionKey({ agent: "devin", id: "s1" })).toBe("devin:s1");
    expect(sessionKey({ agent: "devin", id: "s1", node: "local" })).toBe("local:devin:s1");
    expect(sessionKey({ agent: "cline", id: "s1", node: "node_x" })).toBe("node_x:cline:s1");
  });

  it("projectKey namespaces by node", () => {
    expect(projectKey({ id: "proj_1" })).toBe("proj_1");
    expect(projectKey({ id: "proj_1", node: "node_x" })).toBe("node_x:proj_1");
    expect(projectKey({ id: "proj_1", node: "local" })).toBe("local:proj_1");
  });

  it("the server-issued local id resolves as local once aliased", () => {
    setLocalNodeAlias("node_mine");
    expect(isLocalNode("node_mine")).toBe(true);
    expect(nodeKey("node_mine")).toBe("local");
  });
});

describe("bareProjectId", () => {
  it("strips the node prefix and passes bare ids through", () => {
    expect(bareProjectId("node_x:proj_1")).toBe("proj_1");
    expect(bareProjectId("proj_1")).toBe("proj_1");
  });
});

describe("resolveSession", () => {
  const sessions = [
    { agent: "devin", id: "a", title: "local devin" },
    { agent: "devin", id: "a", node: "node_remote", title: "remote devin" },
    { agent: "cline", id: "c", node: "local", title: "local cline" },
  ];

  it("returns undefined for empty keys", () => {
    expect(resolveSession(sessions, null)).toBeUndefined();
    expect(resolveSession(sessions, undefined)).toBeUndefined();
    expect(resolveSession(sessions, "")).toBeUndefined();
  });

  it("resolves a full node:agent:id key", () => {
    expect(resolveSession(sessions, "node_remote:devin:a")?.title).toBe("remote devin");
    expect(resolveSession(sessions, "local:cline:c")?.title).toBe("local cline");
  });

  it("the local alias resolves a server-issued node id", () => {
    // setLocalNodeAlias ran in the suite above — node_mine maps to local rows.
    const withAlias = [...sessions, { agent: "devin", id: "z", node: "local", title: "aliased" }];
    expect(resolveSession(withAlias, "node_mine:devin:z")?.title).toBe("aliased");
  });

  it("agent:id always means the local node", () => {
    expect(resolveSession(sessions, "devin:a")?.title).toBe("local devin");
    expect(resolveSession(sessions, "cline:c")?.title).toBe("local cline");
  });

  it("a bare id prefers the local row then any node", () => {
    expect(resolveSession(sessions, "a")?.title).toBe("local devin");
    const remoteOnly = [{ agent: "devin", id: "a", node: "node_remote" }];
    expect(resolveSession(remoteOnly, "a")?.node).toBe("node_remote");
    expect(resolveSession(sessions, "ghost")).toBeUndefined();
  });
});

describe("findSessionRow", () => {
  const sessions = [
    { agent: "devin", id: "a" },
    { agent: "cline", id: "a" },
    { agent: "devin", id: "a", node: "node_remote" },
  ];

  it("matches on id plus optional agent and node", () => {
    expect(findSessionRow(sessions, { id: "a" })?.agent).toBe("devin");
    expect(findSessionRow(sessions, { id: "a", agent: "cline" })?.agent).toBe("cline");
    expect(findSessionRow(sessions, { id: "a", node: "node_remote" })?.node).toBe("node_remote");
    expect(
      findSessionRow(sessions, { id: "a", agent: "devin", node: "local" })?.node,
    ).toBeUndefined();
  });

  it("returns undefined when nothing matches", () => {
    expect(findSessionRow(sessions, { id: "zzz" })).toBeUndefined();
    expect(findSessionRow(sessions, { id: "a", agent: "cursor" })).toBeUndefined();
  });
});
