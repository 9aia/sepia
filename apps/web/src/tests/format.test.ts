import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import {
  bareProjectId,
  findSessionRow,
  finishReasonLabel,
  formatCost,
  formatDuration,
  formatUpdated,
  formatUsage,
  isLocalNode,
  isSubAgentOf,
  LOCAL_NODE_ID,
  nodeKey,
  projectKey,
  projectName,
  resolveSession,
  sameSessionKey,
  sessionKey,
  setLocalNodeAlias,
  shouldSyncUrlSelection,
  subAgentsOf,
  usageLabel,
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

describe("sameSessionKey", () => {
  it("is reflexive over identical strings and absent values", () => {
    expect(sameSessionKey("local:devin:a", "local:devin:a")).toBe(true);
    expect(sameSessionKey(null, null)).toBe(true);
    expect(sameSessionKey(undefined, undefined)).toBe(true);
    expect(sameSessionKey(null, "local:devin:a")).toBe(false);
    expect(sameSessionKey("", "local:devin:a")).toBe(false);
    expect(sameSessionKey("local:devin:a", "")).toBe(false);
  });

  it("treats the legacy agent:id form as the local node", () => {
    // The key-shape mismatch behind the selection teleport: a clicked row
    // stores node:agent:id while an old ?session= may carry agent:id.
    expect(sameSessionKey("devin:a", "local:devin:a")).toBe(true);
    expect(sameSessionKey("local:devin:a", "devin:a")).toBe(true);
  });

  it("normalizes the server-issued local node id through the alias", () => {
    // setLocalNodeAlias("node_mine") ran in the suite above.
    expect(sameSessionKey("node_mine:devin:a", "local:devin:a")).toBe(true);
  });

  it("distinguishes different sessions and different nodes", () => {
    expect(sameSessionKey("local:devin:a", "local:devin:b")).toBe(false);
    expect(sameSessionKey("local:devin:a", "local:cline:a")).toBe(false);
    expect(sameSessionKey("local:devin:a", "node_remote:devin:a")).toBe(false);
  });

  it("keeps bare ids agent-ambiguous", () => {
    expect(sameSessionKey("a", "local:devin:a")).toBe(false);
    expect(sameSessionKey("a", "devin:a")).toBe(false);
    expect(sameSessionKey("a", "a")).toBe(true);
  });
});

describe("shouldSyncUrlSelection", () => {
  it("pulls a present session param into the store", () => {
    expect(shouldSyncUrlSelection("local:devin:b", "local:devin:a")).toBe(true);
    expect(shouldSyncUrlSelection("local:devin:a", null)).toBe(true);
  });

  it("never clears a selection when the param is absent or empty", () => {
    // Regression: a URL read of no ?session= must not null the store —
    // the auto-select effect then snapped the selection to the first row.
    expect(shouldSyncUrlSelection(undefined, "local:devin:a")).toBe(false);
    expect(shouldSyncUrlSelection("", "local:devin:a")).toBe(false);
  });

  it("skips equivalent key forms so they can't ping-pong", () => {
    expect(shouldSyncUrlSelection("devin:a", "local:devin:a")).toBe(false);
    expect(shouldSyncUrlSelection("local:devin:a", "devin:a")).toBe(false);
    expect(shouldSyncUrlSelection("local:devin:a", "local:devin:a")).toBe(false);
  });
});

describe("formatCost", () => {
  it("formats cents and hides non-positive costs", () => {
    expect(formatCost(0.0234)).toBe("$0.02");
    expect(formatCost(1.5)).toBe("$1.50");
    expect(formatCost(0)).toBe("");
    expect(formatCost(-1)).toBe("");
    expect(formatCost(Number.NaN)).toBe("");
  });

  it("keeps sub-cent precision instead of rounding to $0.00", () => {
    expect(formatCost(0.0042)).toBe("$0.0042");
  });
});

describe("usageLabel", () => {
  it("compacts input/output with arrows", () => {
    expect(usageLabel({ input: 1200, output: 340 })).toBe("↑1.2K ↓340");
  });

  it("appends cost when priced above zero", () => {
    expect(usageLabel({ input: 1200, output: 340, cost: 0.023 })).toBe("↑1.2K ↓340 · $0.02");
    expect(usageLabel({ input: 1200, output: 340, cost: 0 })).toBe("↑1.2K ↓340");
  });
});

describe("formatUsage", () => {
  it("lists input and output with grouped digits", () => {
    expect(formatUsage({ input: 12345, output: 678 })).toBe("12,345 input · 678 output");
  });

  it("includes cache tiers, thinking, and cost when recorded", () => {
    expect(
      formatUsage({
        input: 100,
        output: 50,
        cacheRead: 800,
        cacheWrite: 12,
        thinking: 56,
        cost: 0.02,
      }),
    ).toBe("100 input · 50 output · 800 cache read · 12 cache write · 56 thinking · $0.02");
  });

  it("omits zero/absent tiers", () => {
    expect(formatUsage({ input: 1, output: 2, cacheRead: 0 })).toBe("1 input · 2 output");
  });
});

describe("formatDuration", () => {
  it("formats sub-second, seconds, and minutes", () => {
    expect(formatDuration(450)).toBe("450ms");
    expect(formatDuration(1200)).toBe("1.2s");
    expect(formatDuration(59_999)).toBe("60.0s");
    expect(formatDuration(64_000)).toBe("1m 4s");
    expect(formatDuration(120_000)).toBe("2m");
  });

  it("returns empty for bad input", () => {
    expect(formatDuration(-5)).toBe("");
    expect(formatDuration(Number.NaN)).toBe("");
  });
});

describe("finishReasonLabel", () => {
  it("quiets the ordinary endings", () => {
    expect(finishReasonLabel("stop")).toBeNull();
    expect(finishReasonLabel("end_turn")).toBeNull();
    expect(finishReasonLabel("tool_calls")).toBeNull();
    expect(finishReasonLabel(undefined)).toBeNull();
    expect(finishReasonLabel("")).toBeNull();
  });

  it("surfaces terminal reasons", () => {
    expect(finishReasonLabel("length")).toBe("length");
    expect(finishReasonLabel("error")).toBe("error");
    expect(finishReasonLabel("content_filter")).toBe("content_filter");
  });
});

describe("isSubAgentOf / subAgentsOf", () => {
  // Adapters write the parent's bare session id — cline slices it out of
  // the child's own id (`<parent>__agent_<name>`), devin reads
  // subagent_heads.session_id. Both rows share one agent store + node.
  const sessions = [
    { agent: "cline", id: "parent-1", title: "local parent" },
    {
      agent: "cline",
      id: "parent-1__agent_scout",
      parentSessionId: "parent-1",
      title: "local child",
    },
    { agent: "devin", id: "parent-1", title: "same id, other store" },
    { agent: "cline", id: "remote-parent", node: "node_remote", title: "remote parent" },
    {
      agent: "cline",
      id: "remote-child",
      node: "node_remote",
      parentSessionId: "remote-parent",
      title: "remote child",
    },
    {
      agent: "cline",
      id: "keyed-child",
      parentSessionId: "node_remote:cline:remote-parent",
      title: "keyed ref",
    },
    { agent: "cline", id: "stray", parentSessionId: "ghost", title: "missing parent" },
    { agent: "cline", id: "self", parentSessionId: "self", title: "self-parented" },
    { agent: "cline", id: "root", title: "no parent" },
  ];

  it("matches a bare parent id within the same agent store and node", () => {
    expect(isSubAgentOf(sessions, sessions[1], sessions[0])).toBe(true);
    expect(isSubAgentOf(sessions, sessions[4], sessions[3])).toBe(true);
  });

  it("rejects a same-id parent in another agent's store", () => {
    // The cline child's bare "parent-1" is not the devin row's parentage.
    expect(isSubAgentOf(sessions, sessions[1], sessions[2])).toBe(false);
  });

  it("rejects a same-id parent on another node", () => {
    const localParent = { agent: "cline", id: "remote-parent" };
    expect(isSubAgentOf(sessions, sessions[4], localParent)).toBe(false);
  });

  it("resolves keyed node:agent:id and agent:id refs", () => {
    expect(isSubAgentOf(sessions, sessions[5], sessions[3])).toBe(true);
    const legacyRef = { agent: "cline", id: "legacy", parentSessionId: "cline:parent-1" };
    expect(isSubAgentOf(sessions, legacyRef, sessions[0])).toBe(true);
  });

  it("maps the server-issued local node id through the alias", () => {
    setLocalNodeAlias("node_mine");
    const aliased = {
      agent: "cline",
      id: "aliased-child",
      parentSessionId: "node_mine:cline:parent-1",
    };
    expect(isSubAgentOf(sessions, aliased, sessions[0])).toBe(true);
  });

  it("returns no match for absent, empty, or dangling refs", () => {
    expect(isSubAgentOf(sessions, sessions[8], sessions[0])).toBe(false);
    expect(isSubAgentOf(sessions, sessions[6], sessions[0])).toBe(false);
    const empty = { agent: "cline", id: "e", parentSessionId: "" };
    expect(isSubAgentOf(sessions, empty, sessions[0])).toBe(false);
  });

  it("subAgentsOf lists the children and skips unrelated rows", () => {
    const kids = subAgentsOf(sessions, sessions[0]);
    expect(kids.map((s) => s.id)).toEqual(["parent-1__agent_scout"]);
    expect(subAgentsOf(sessions, sessions[3]).map((s) => s.id)).toEqual([
      "remote-child",
      "keyed-child",
    ]);
  });

  it("never lists a session as its own child and handles no parent", () => {
    expect(subAgentsOf(sessions, sessions[7])).toEqual([]);
    expect(subAgentsOf(sessions, undefined)).toEqual([]);
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
