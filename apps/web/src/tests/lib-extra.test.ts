/**
 * Batch branch coverage for lib helpers — validation fallbacks, merges,
 * key parsing, size formatting and the store-backed migrations.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), loading: vi.fn(() => "l1") },
}));

import { toast } from "sonner";
import { toastError, toastLoading, toastSuccess } from "../lib/toast";
import { partToBlock, formatAttachmentSize } from "../lib/attachments";
import { parseErrorPayload } from "../lib/errorPayload";
import { parseAgentKey, parseCatalogKey, sameCatalogKey } from "../lib/format";
import { parseSystemContext } from "../lib/systemContext";
import { getToken } from "../lib/token";
import { spanNodeLabel, nodesStore } from "../lib/nodes";
import { resumeTargets } from "../lib/resume";
import { treeFromSessions, SESSION_TREE_ROOT } from "../lib/sessionTree";
import { attachmentViews } from "../lib/blocks";
import type { SessionSummary } from "../lib/types";

const store = new Map<string, string>();
const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("toast variants", () => {
  it("loading + resolve-in-place + error details", () => {
    expect(toastLoading("Working")).toBe("l1");
    expect(vi.mocked(toast.loading)).toHaveBeenCalledWith("Working");

    toastSuccess("Done", "l1");
    expect(toast.success).toHaveBeenCalledWith("Done", { id: "l1" });

    toastError("Failed", new Error("bad disk"), "l1");
    expect(toast.error).toHaveBeenCalledWith("Failed", {
      description: "bad disk",
      id: "l1",
    });
    toastError("Failed", "not an error");
    expect(toast.error).toHaveBeenCalledWith("Failed", { description: undefined });
  });
});

describe("partToBlock / formatAttachmentSize", () => {
  it("maps every content type and formats sizes", () => {
    expect(partToBlock({ type: "text", text: "hi" })).toEqual({ type: "text", text: "hi" });
    expect(partToBlock({ type: "image", data: "AA", mimeType: "image/png" })).toEqual({
      type: "image",
      data: "AA",
      mimeType: "image/png",
    });
    expect(
      partToBlock({ type: "image", data: "AA", mimeType: "image/png", uri: "file:///x.png" }),
    ).toEqual({ type: "image", data: "AA", mimeType: "image/png", uri: "file:///x.png" });
    expect(partToBlock({ type: "audio", data: "BB", mimeType: "audio/mp3" })).toEqual({
      type: "audio",
      data: "BB",
      mimeType: "audio/mp3",
    });
    expect(partToBlock({ type: "resource", resource: { uri: "file:///a", text: "t" } })).toEqual({
      type: "file",
      uri: "file:///a",
      text: "t",
    });
    expect(
      partToBlock({
        type: "resource",
        resource: { uri: "file:///b", blob: "BB", mimeType: "application/x" },
      }),
    ).toEqual({ type: "file", uri: "file:///b", mimeType: "application/x", data: "BB" });
    expect(partToBlock({ type: "resource_link", uri: "https://x", name: "n" })).toEqual({
      type: "file",
      uri: "https://x",
      name: "n",
    });

    expect(formatAttachmentSize(512)).toBe("512 B");
    expect(formatAttachmentSize(2048)).toBe("2.0 KB");
    expect(formatAttachmentSize(3 * 1024 * 1024)).toBe("3.0 MB");
    // size formatting inside the views: NaN/negative → no detail string
    const views = attachmentViews([
      { type: "file", uri: "file:///a", size: Number.NaN },
      { type: "file", uri: "file:///b", size: 2048 },
    ] as never);
    const file = views[1];
    expect(file?.kind === "file" ? file.detail : "").toContain("KB");
  });
});

describe("parseErrorPayload", () => {
  it("handles string errors, nested codes, and rejects non-JSON", () => {
    expect(parseErrorPayload('{"error":"boom","code":"locked"}')).toEqual({
      message: "boom",
      code: "locked",
    });
    expect(parseErrorPayload('{"error":{"message":"inner"},"code":"outer"}')).toEqual({
      message: "inner",
      code: "outer",
    });
    expect(parseErrorPayload('{"error":{"message":"inner","code":"inner-code"}}')).toEqual({
      message: "inner",
      code: "inner-code",
    });
    expect(parseErrorPayload("not json")).toBeNull();
    expect(parseErrorPayload("[1,2]")).toBeNull();
    expect(parseErrorPayload("{}")).toBeNull();
  });
});

describe("key parsing", () => {
  it("rejects malformed agent/catalog keys and matches local aliases", () => {
    expect(parseAgentKey("a")).toBeNull();
    expect(parseAgentKey("a:b:c")).toBeNull();
    expect(parseAgentKey(":b")).toBeNull();
    expect(parseCatalogKey("a:b")).toBeNull();
    expect(parseCatalogKey("a:b:")).toBeNull();
    expect(parseCatalogKey("n:a:m:extra")).toEqual({ node: "n", agent: "a", model: "m:extra" });
    expect(sameCatalogKey(null, "a:b:c")).toBe(false);
    expect(sameCatalogKey("a:b:c", "bad")).toBe(false);
    expect(sameCatalogKey("local:a:m", "local:a:m")).toBe(true);
  });
});

describe("systemContext rule merging", () => {
  it("a later block carries the body an earlier occurrence lacked", () => {
    const content = [
      "<rules>",
      '<rule name="r" path="/p.md"></rule>',
      "</rules>",
      "middle text",
      "<rules>",
      '<rule name="r" path="/p.md">real content</rule>',
      '<rule name="r2" path="/q.md">second</rule>',
      "</rules>",
    ].join("\n");
    const ctx = parseSystemContext([{ role: "system", nodeId: 0, content, createdAt: 0 }] as never);
    const rule = ctx.rules.find((r) => r.name === "r");
    expect(rule?.content).toBe("real content");
    expect(ctx.rules.find((r) => r.name === "r2")?.content).toBe("second");
    expect(ctx.promptText).toContain("middle text");
  });
});

describe("token migration", () => {
  it("migrates a legacy bare-string store onto the current base", () => {
    // token.test.ts keeps its own stub; here the TOKEN_KEY holds a bare string
    store.set("sepia:token", "legacy-token");
    expect(getToken()).toBe("legacy-token");
    // migrated entry persisted under the current base
    // no localNodeUrl configured → the base is "" and the entry lands there
    expect(JSON.parse(store.get("sepia:token") ?? "{}")).toHaveProperty("");
  });
});

describe("spanNodeLabel", () => {
  it("labels local, self, peer and unknown node ids", () => {
    expect(["local", "this machine"]).toContain(spanNodeLabel("local"));
    nodesStore.setState((prev) => ({
      ...prev,
      self: { id: "self-123", name: "my box" } as never,
      peers: [
        {
          id: "peer-9",
          name: "peer name",
          alias: "prod",
          enabled: true,
          via: "direct",
          url: "x",
        } as never,
      ],
    }));
    expect(spanNodeLabel("self-123")).toBe("my box");
    expect(spanNodeLabel("peer-9")).toBe("prod");
    expect(spanNodeLabel("a-very-long-unknown-node-id")).toBe("a-very-long-un…");
    expect(spanNodeLabel("")).toBe("local");
    nodesStore.setState((prev) => ({ ...prev, self: null, peers: [] }));
  });
});

describe("resumeTargets", () => {
  it("drops the session's own node+agent from the target list", () => {
    const nodes = [
      { label: "This machine", agents: ["devin", "cline"] },
      { node: "peer-1", label: "peer", agents: ["devin"] },
    ];
    const targets = resumeTargets(nodes, { agent: "devin", node: undefined });
    const local = targets.find((t) => t.node === undefined);
    expect(local?.agents).toEqual(["cline"]);
    expect(targets.some((t) => t.node === "peer-1")).toBe(true);
  });
});

describe("sessionTree grouped mode", () => {
  const makeSession = (over: Partial<SessionSummary>): SessionSummary => ({
    id: "x",
    title: "t",
    cwd: "/w",
    agent: "devin",
    updatedAt: "2026-01-01",
    locked: false,
    lockHolderPid: null,
    source: "devin",
    busy: false,
    pinned: false,
    archived: false,
    projectIds: [],
    model: null,
    spans: [],
    ...over,
  });

  it("groups sessions under node headers with local first", () => {
    const { dataMap, childrenMap } = treeFromSessions(
      [
        makeSession({ id: "s-local", node: undefined }),
        makeSession({ id: "s-peer", node: "peer-9" }),
      ],
      { groupByNode: true },
    );
    const roots = childrenMap.get(SESSION_TREE_ROOT) ?? [];
    expect(roots[0]).toBe("node:local");
    expect(roots).toContain("node:peer-9");
    expect(dataMap.get("node:peer-9")).toMatchObject({ kind: "node", node: "peer-9" });
  });

  it("ungrouped mode keeps a flat dir tree", () => {
    const { rootChildren } = treeFromSessions(
      [makeSession({ id: "a", cwd: "/work/app" }), makeSession({ id: "b", cwd: "/work/app" })],
      {},
    );
    expect(rootChildren.length).toBeGreaterThan(0);
  });
});
