import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  addPeer,
  getPeers,
  isMultiNode,
  listAllAgents,
  listAllProjects,
  listAllSessions,
  nodeName,
  nodesStore,
  nodeTarget,
  normalizeNodeUrl,
  peerTarget,
  refreshSelf,
  removePeer,
  removePeerById,
  upsertPeer,
  type PeerNode,
} from "../lib/nodes";
import { getNode, listAgents, listProjects, listSessions } from "../lib/api";
import { getToken } from "../lib/token";
import type { Project, SessionSummary } from "../lib/types";

vi.mock("../lib/api", () => ({
  getNode: vi.fn(),
  listSessions: vi.fn(),
  listProjects: vi.fn(),
  listAgents: vi.fn(),
}));

const mockedGetNode = vi.mocked(getNode);
const mockedListSessions = vi.mocked(listSessions);
const mockedListProjects = vi.mocked(listProjects);
const mockedListAgents = vi.mocked(listAgents);

const store = new Map<string, string>();

const peer = (id: string, name = id): PeerNode => ({
  id,
  name,
  url: `https://${id}.example`,
  token: `tok-${id}`,
});

const session = (id: string, projectIds: string[] = []): SessionSummary => ({
  id,
  title: id,
  cwd: "/w",
  agent: "devin",
  updatedAt: "2024-01-01T00:00:00.000Z",
  locked: false,
  lockHolderPid: null,
  source: "test",
  busy: false,
  pinned: false,
  archived: false,
  projectIds,
  model: null,
  spans: [],
});

beforeEach(() => {
  vi.clearAllMocks();
  store.clear();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, String(value));
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
  });
  nodesStore.setState(() => ({ self: null, peers: [] }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("normalizeNodeUrl", () => {
  it("prepends http:// to bare hosts and strips trailing slashes", () => {
    expect(normalizeNodeUrl("thinkpad:8787")).toBe("http://thinkpad:8787");
    expect(normalizeNodeUrl("https://peer.example/")).toBe("https://peer.example");
    expect(normalizeNodeUrl("  http://a.example//  ")).toBe("http://a.example");
  });

  it("returns just the origin for URLs with paths", () => {
    expect(normalizeNodeUrl("https://peer.example/some/path?q=1")).toBe("https://peer.example");
  });

  it("rejects unparseable input", () => {
    expect(() => normalizeNodeUrl("")).toThrow();
    expect(() => normalizeNodeUrl("   ")).toThrow();
    expect(() => normalizeNodeUrl("http://bad host")).toThrow();
  });
});

describe("peer list helpers", () => {
  it("upsertPeer replaces by id or url and appends new peers", () => {
    const base = [peer("a"), peer("b")];
    expect(upsertPeer(base, peer("c")).map((p) => p.id)).toEqual(["a", "b", "c"]);
    // Same id, new url → replaced in place order-wise (rest + new).
    expect(upsertPeer(base, { ...peer("a"), url: "https://a2.example" }).map((p) => p.id)).toEqual([
      "b",
      "a",
    ]);
    // Same url, different id → the old url's owner is evicted.
    expect(
      upsertPeer(base, { id: "z", name: "z", url: "https://a.example", token: null }).map(
        (p) => p.id,
      ),
    ).toEqual(["b", "z"]);
  });

  it("removePeerById filters by id only", () => {
    expect(removePeerById([peer("a"), peer("b")], "a").map((p) => p.id)).toEqual(["b"]);
    expect(removePeerById([peer("a")], "zzz")).toHaveLength(1);
  });
});

describe("addPeer / removePeer / refreshSelf", () => {
  it("addPeer registers the probed descriptor and persists it", async () => {
    mockedGetNode.mockResolvedValue({
      id: "node_1",
      name: "thinkpad",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    const added = await addPeer("https://thinkpad.example/", " secret ");
    expect(added).toEqual({
      id: "node_1",
      name: "thinkpad",
      url: "https://thinkpad.example",
      token: "secret",
    });
    expect(getPeers()).toHaveLength(1);
    expect(isMultiNode()).toBe(true);
    expect(JSON.parse(store.get("sepia:nodes") ?? "[]")).toHaveLength(1);

    removePeer("node_1");
    expect(getPeers()).toEqual([]);
    expect(isMultiNode()).toBe(false);
    expect(JSON.parse(store.get("sepia:nodes") ?? "[]")).toEqual([]);
  });

  it("a blank token normalizes to null", async () => {
    mockedGetNode.mockResolvedValue({
      id: "n",
      name: "n",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    expect((await addPeer("http://h", "   ")).token).toBeNull();
  });

  it("refuses to register the local node as a peer", async () => {
    mockedGetNode.mockResolvedValue({
      id: "node_self",
      name: "me",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    nodesStore.setState(() => ({
      self: {
        id: "node_self",
        name: "me",
        version: "1",
        protocol: 1,
        agents: [],
        capabilities: [],
      },
      peers: [],
    }));
    await expect(addPeer("http://x", "")).rejects.toThrow("already in the list");
    expect(getPeers()).toEqual([]);
  });

  it("refreshSelf stores the local descriptor", async () => {
    mockedGetNode.mockResolvedValue({
      id: "node_local",
      name: "laptop",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    const self = await refreshSelf();
    expect(self.name).toBe("laptop");
    expect(nodesStore.state.self?.id).toBe("node_local");
  });
});

describe("nodeTarget / nodeName", () => {
  it("resolves local, peer, and unknown node ids", () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p")] }));
    expect(nodeTarget(undefined)).toEqual({ baseUrl: "", token: getToken() });
    expect(nodeTarget("local")).toEqual({ baseUrl: "", token: getToken() });
    expect(nodeTarget("node_p")).toEqual({
      baseUrl: "https://node_p.example",
      token: "tok-node_p",
      timeoutMs: 3_000,
    });
    // Unknown peers get a deliberately unreachable target.
    expect(nodeTarget("node_ghost").baseUrl).toBe("http://node_ghost.invalid");
  });

  it("peerTarget carries the peer credentials with the fan-out timeout", () => {
    expect(peerTarget(peer("node_p"))).toEqual({
      baseUrl: "https://node_p.example",
      token: "tok-node_p",
      timeoutMs: 3_000,
    });
  });

  it("nodeName prefers the self name, falls back gracefully", () => {
    expect(nodeName(undefined)).toBe("this machine");
    nodesStore.setState(() => ({
      self: {
        id: "node_self",
        name: "laptop",
        version: "1",
        protocol: 1,
        agents: [],
        capabilities: [],
      },
      peers: [peer("node_p", "Thinkpad")],
    }));
    expect(nodeName(undefined)).toBe("laptop");
    expect(nodeName("local")).toBe("laptop");
    expect(nodeName("node_p")).toBe("Thinkpad");
    expect(nodeName("node_ghost")).toBe("node_ghost");
  });
});

describe("fan-out fetches", () => {
  it("listAllSessions returns untagged local rows in single-node mode", async () => {
    mockedListSessions.mockResolvedValue([session("a", ["proj_1"])]);
    const rows = await listAllSessions();
    expect(rows[0]?.node).toBeUndefined();
    expect(rows[0]?.projectIds).toEqual(["proj_1"]);
  });

  it("listAllSessions tags local + peer rows and namespaces projectIds", async () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p")] }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1", ["p1"])] : [session("peer-1", ["p2"])],
    );
    const rows = await listAllSessions();
    expect(rows.map((r) => `${r.node}:${r.id}`)).toEqual(["local:local-1", "node_p:peer-1"]);
    expect(rows[0]?.projectIds).toEqual(["local:p1"]);
    expect(rows[1]?.projectIds).toEqual(["node_p:p2"]);
  });

  it("a dead peer contributes nothing; a dead local node propagates", async () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p")] }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1")] : Promise.reject(new Error("peer down")),
    );
    const rows = await listAllSessions();
    expect(rows).toHaveLength(1);
    expect(rows[0]?.id).toBe("local-1");

    mockedListSessions.mockImplementation(async () => Promise.reject(new Error("local down")));
    await expect(listAllSessions()).rejects.toThrow("local down");
  });

  it("listAllProjects tags rows by node and tolerates peer failure", async () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p")] }));
    const project = (id: string): Project => ({ id, name: id });
    mockedListProjects.mockImplementation(async (target) =>
      target?.baseUrl === "" ? { projects: [project("p1")] } : Promise.reject(new Error("down")),
    );
    const rows = await listAllProjects();
    expect(rows).toEqual([{ id: "p1", name: "p1", node: "local" }]);

    mockedListProjects.mockImplementation(async (target) =>
      target?.baseUrl === "" ? { projects: [project("p1")] } : { projects: [project("p2")] },
    );
    expect((await listAllProjects()).map((p) => `${p.node}:${p.id}`)).toEqual([
      "local:p1",
      "node_p:p2",
    ]);
  });

  it("listAllAgents dedupes by id with local winning, tolerates peer failure", async () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p"), peer("node_dead")] }));
    mockedListAgents.mockImplementation(async (target) => {
      if (target?.baseUrl === "") return [{ id: "devin", label: "Devin local" }];
      if (target?.baseUrl.includes("node_dead")) throw new Error("down");
      return [
        { id: "devin", label: "Devin peer" },
        { id: "cline", label: "Cline" },
      ];
    });
    const agents = await listAllAgents();
    expect(agents).toEqual([
      { id: "devin", label: "Devin local" },
      { id: "cline", label: "Cline" },
    ]);
  });
});
