import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  addGatewayPeer,
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
  normalizePeer,
  pairGatewayPeer,
  pairPeer,
  peerTarget,
  refreshSelf,
  removePeer,
  removePeerById,
  removePeerEntry,
  setPeerAlias,
  upsertPeer,
  type PeerNode,
} from "../lib/nodes";
import { getNode, listAgents, listProjects, listSessions, pairNode } from "../lib/api";
import { createServer, deleteServer, updateServer } from "../lib/servers";
import { getToken } from "../lib/token";
import type { Project, SessionSummary } from "../lib/types";

vi.mock("../lib/api", () => ({
  getNode: vi.fn(),
  listSessions: vi.fn(),
  listProjects: vi.fn(),
  listAgents: vi.fn(),
  pairNode: vi.fn(),
}));

// Keep the real gatewayTarget (target-shape assertions use it); stub the
// managed-registry calls that would otherwise hit fetch.
vi.mock("../lib/servers", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/servers")>();
  return {
    ...actual,
    createServer: vi.fn(),
    updateServer: vi.fn(),
    deleteServer: vi.fn(),
  };
});

const mockedGetNode = vi.mocked(getNode);
const mockedListSessions = vi.mocked(listSessions);
const mockedListProjects = vi.mocked(listProjects);
const mockedListAgents = vi.mocked(listAgents);
const mockedPairNode = vi.mocked(pairNode);
const mockedCreateServer = vi.mocked(createServer);
const mockedUpdateServer = vi.mocked(updateServer);
const mockedDeleteServer = vi.mocked(deleteServer);

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

  it("pairPeer redeems the code for a token, then registers like addPeer", async () => {
    mockedPairNode.mockResolvedValue({ token: "sepia_issued" });
    mockedGetNode.mockResolvedValue({
      id: "node_1",
      name: "thinkpad",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: ["pairing"],
    });
    const added = await pairPeer("thinkpad:8787", " abcd-efgh ");
    expect(mockedPairNode).toHaveBeenCalledWith("abcd-efgh", {
      baseUrl: "http://thinkpad:8787",
      token: null,
      timeoutMs: 5_000,
    });
    expect(added).toEqual({
      id: "node_1",
      name: "thinkpad",
      url: "http://thinkpad:8787",
      token: "sepia_issued",
    });
    expect(getPeers()).toHaveLength(1);
  });

  it("pairPeer propagates a rejected code without registering", async () => {
    mockedPairNode.mockRejectedValue(new Error("That code didn't work"));
    await expect(pairPeer("http://h", "bad")).rejects.toThrow("That code didn't work");
    expect(getPeers()).toEqual([]);
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

  it("peer alias overrides the self-reported name", () => {
    nodesStore.setState(() => ({ self: null, peers: [peer("node_p", "Thinkpad")] }));
    setPeerAlias("node_p", "work laptop");
    expect(nodeName("node_p")).toBe("work laptop");
    // Clearing the alias reverts to the name.
    setPeerAlias("node_p", "   ");
    expect(nodeName("node_p")).toBe("Thinkpad");
    expect(nodesStore.state.peers[0]?.alias).toBeUndefined();
  });

  it("normalizePeer round-trips and validates alias", () => {
    expect(normalizePeer({ ...peer("a"), alias: "desk" })?.alias).toBe("desk");
    expect(normalizePeer({ ...peer("a"), alias: "" })?.alias).toBeUndefined();
    expect(normalizePeer({ ...peer("a"), alias: 5 })?.alias).toBeUndefined();
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

describe("gateway peers (via: gateway)", () => {
  const descriptor = {
    id: "node_remote",
    name: "remote-box",
    version: "1",
    protocol: 1,
    agents: [],
    capabilities: [],
  };
  const managed = {
    id: "srv_1",
    label: "remote.example",
    host: "remote.example",
    port: 8787,
    scheme: "http" as const,
    auth: null,
    ssh: null,
  };
  const gatewayPeer = (): PeerNode => ({
    id: "node_remote",
    name: "remote-box",
    url: "http://remote.example:8787",
    token: null,
    via: "gateway",
    serverId: "srv_1",
  });

  it("peerTarget resolves to this node's /api/gateway/<serverId> forward", () => {
    expect(peerTarget(gatewayPeer())).toEqual({
      baseUrl: "/api/gateway/srv_1",
      token: getToken(),
      timeoutMs: 12_000,
    });
    // The peer's browser-held token is never used — calls authenticate with
    // the local node's token and the server injects the stored one upstream.
  });

  it("nodeTarget flips gateway peers, keeps direct peers direct", () => {
    nodesStore.setState(() => ({ self: null, peers: [gatewayPeer(), peer("node_direct")] }));
    expect(nodeTarget("node_remote").baseUrl).toBe("/api/gateway/srv_1");
    expect(nodeTarget("node_direct")).toEqual({
      baseUrl: "https://node_direct.example",
      token: "tok-node_direct",
      timeoutMs: 3_000,
    });
  });

  it("normalizePeer keeps via+serverId, degrades a gateway entry without one", () => {
    const stored = normalizePeer({
      id: "n",
      url: "http://h:8787",
      via: "gateway",
      serverId: "srv_9",
    });
    expect(stored?.via).toBe("gateway");
    expect(stored?.serverId).toBe("srv_9");

    const degraded = normalizePeer({ id: "n", url: "http://h:8787", via: "gateway" });
    expect(degraded?.via).toBeUndefined();
    expect(degraded?.serverId).toBeUndefined();

    // Legacy entries without the fields stay direct.
    expect(normalizePeer({ id: "n", url: "http://h", token: "t" })?.via).toBeUndefined();
  });

  it("addGatewayPeer registers the credential server-side, then probes through the gateway", async () => {
    mockedCreateServer.mockResolvedValue(managed);
    mockedGetNode.mockResolvedValue(descriptor);

    const added = await addGatewayPeer("http://remote.example:8787", "peer-secret");
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http",
      auth: { type: "token", secret: "peer-secret" },
      ssh: null,
    });
    // The probe travels through the fresh gateway hop, not the peer URL.
    expect(mockedGetNode).toHaveBeenCalledWith({
      baseUrl: "/api/gateway/srv_1",
      token: getToken(),
      timeoutMs: 5_000,
    });
    expect(added).toEqual({
      id: "node_remote",
      name: "remote-box",
      url: "http://remote.example:8787",
      token: null,
      via: "gateway",
      serverId: "srv_1",
    });
    const persisted = JSON.parse(store.get("sepia:nodes") ?? "[]") as PeerNode[];
    expect(persisted[0]?.via).toBe("gateway");
    expect(persisted[0]?.serverId).toBe("srv_1");
  });

  it("addGatewayPeer propagates an https peer URL as scheme+port in the managed entry", async () => {
    mockedCreateServer.mockResolvedValue({ ...managed, scheme: "https", port: 443 });
    mockedGetNode.mockResolvedValue(descriptor);

    const added = await addGatewayPeer("https://remote.example/", "peer-secret");
    // A bare https:// URL implies :443 and stores scheme: "https" so the
    // gateway proxy fetches the TLS upstream instead of plaintext.
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "remote.example",
      host: "remote.example",
      port: 443,
      scheme: "https",
      auth: { type: "token", secret: "peer-secret" },
      ssh: null,
    });
    expect(added.url).toBe("https://remote.example");
  });

  it("pairGatewayPeer keeps the https scheme when writing the issued token back", async () => {
    mockedCreateServer.mockResolvedValue({ ...managed, scheme: "https", port: 443 });
    mockedPairNode.mockResolvedValue({ token: "sepia_issued" });
    mockedUpdateServer.mockResolvedValue({
      ...managed,
      scheme: "https",
      port: 443,
      auth: { type: "token", secret: "••••••••" },
    });
    mockedGetNode.mockResolvedValue(descriptor);

    await pairGatewayPeer("https://remote.example:8443", "7K2M-9PQX");
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "remote.example",
      host: "remote.example",
      port: 8443,
      scheme: "https",
      auth: null,
      ssh: null,
    });
    expect(mockedUpdateServer).toHaveBeenCalledWith("srv_1", {
      label: "remote.example",
      host: "remote.example",
      port: 8443,
      scheme: "https",
      auth: { type: "token", secret: "sepia_issued" },
      ssh: null,
    });
  });

  it("addGatewayPeer drops the managed entry when the probe fails", async () => {
    mockedCreateServer.mockResolvedValue(managed);
    mockedDeleteServer.mockResolvedValue(undefined);
    mockedGetNode.mockRejectedValue(new Error("401 from peer"));

    await expect(addGatewayPeer("http://remote.example:8787", "bad")).rejects.toThrow(
      "401 from peer",
    );
    expect(mockedDeleteServer).toHaveBeenCalledWith("srv_1");
    expect(getPeers()).toEqual([]);
  });

  it("pairGatewayPeer redeems the code through the gateway, then stores the issued token", async () => {
    mockedCreateServer.mockResolvedValue(managed);
    mockedPairNode.mockResolvedValue({ token: "sepia_issued" });
    mockedUpdateServer.mockResolvedValue({
      ...managed,
      auth: { type: "token", secret: "••••••••" },
    });
    mockedGetNode.mockResolvedValue(descriptor);

    const added = await pairGatewayPeer("http://remote.example:8787", "7K2M-9PQX");
    // The entry starts credential-less so the unauthenticated /api/pair forwards.
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http",
      auth: null,
      ssh: null,
    });
    expect(mockedPairNode).toHaveBeenCalledWith(
      "7K2M-9PQX",
      { baseUrl: "/api/gateway/srv_1", token: getToken(), timeoutMs: 12_000 },
      { forwardTargetAuth: true },
    );
    expect(mockedUpdateServer).toHaveBeenCalledWith("srv_1", {
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http",
      auth: { type: "token", secret: "sepia_issued" },
      ssh: null,
    });
    expect(added.via).toBe("gateway");
    expect(added.serverId).toBe("srv_1");
    expect(getPeers()).toHaveLength(1);
  });

  it("removePeerEntry deletes the managed credential for gateway peers only", async () => {
    nodesStore.setState(() => ({ self: null, peers: [gatewayPeer(), peer("node_direct")] }));
    mockedDeleteServer.mockResolvedValue(undefined);

    await removePeerEntry("node_remote");
    expect(mockedDeleteServer).toHaveBeenCalledWith("srv_1");
    expect(getPeers().map((p) => p.id)).toEqual(["node_direct"]);

    mockedDeleteServer.mockClear();
    await removePeerEntry("node_direct");
    expect(mockedDeleteServer).not.toHaveBeenCalled();
    expect(getPeers()).toEqual([]);
  });

  it("removePeerEntry still removes the peer when the managed entry is already gone", async () => {
    nodesStore.setState(() => ({ self: null, peers: [gatewayPeer()] }));
    mockedDeleteServer.mockRejectedValue(new Error("Unknown server"));

    await removePeerEntry("node_remote");
    expect(getPeers()).toEqual([]);
  });
});
