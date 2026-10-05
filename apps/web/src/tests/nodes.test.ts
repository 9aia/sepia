import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  addGatewayPeer,
  addPeer,
  getPeers,
  isLocalNodeEnabled,
  isMultiNode,
  isPeerEnabled,
  isThisMachine,
  listAllAgents,
  listAllProjects,
  listAllSessions,
  localNodeAddress,
  nodeName,
  nodesStore,
  nodeTarget,
  normalizeNodeUrl,
  normalizePeer,
  pairGatewayPeer,
  pairPeer,
  parseNodeAddress,
  peerSecret,
  peerTarget,
  peerUrlParts,
  refreshSelf,
  removePeer,
  removePeerById,
  removePeerEntry,
  setLocalNodeEnabled,
  setLocalNodeUrl,
  setPeerAlias,
  setPeerEnabled,
  updatePeerEntry,
  upsertPeer,
  type PeerNode,
} from "../lib/nodes";
import { addCredential, credentialById, credentialsStore } from "../lib/credentials";
import { settingsStore } from "../lib/settings";
import { getNode, listAgents, listProjects, listSessions, pairNode } from "../lib/api";
import { createServer, deleteServer, listServers, SECRET_MASK, updateServer } from "../lib/servers";
import { getToken } from "../lib/token";
import { localTarget } from "../lib/targets";
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
    listServers: vi.fn(),
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
const mockedListServers = vi.mocked(listServers);

const store = new Map<string, string>();

// A peer fixture linked to a freshly filed credential — the credentialId
// → secret resolution mirrors what a stored registry produces on load.
const peer = (id: string, name = id): PeerNode => ({
  id,
  name,
  url: `https://${id}.example`,
  credentialId: addCredential({ label: name, secret: `tok-${id}` }).id,
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
  nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [] }));
  credentialsStore.setState(() => []);
  settingsStore.setState((prev) => ({
    ...prev,
    localNodeEnabled: true,
    localNodeName: null,
    localNodeUrl: null,
  }));
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
      upsertPeer(base, { id: "z", name: "z", url: "https://a.example" }).map((p) => p.id),
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
    const added = await addPeer("https://thinkpad.example/", { secret: " secret " });
    expect(added).toEqual({
      id: "node_1",
      name: "thinkpad",
      url: "https://thinkpad.example",
      credentialId: expect.any(String),
    });
    // The secret moved into the credential store — peerTarget resolves it.
    expect(peerSecret(added)).toBe("secret");
    expect(peerTarget(added)).toEqual({
      baseUrl: "https://thinkpad.example",
      token: "secret",
      timeoutMs: 3_000,
    });
    expect(getPeers()).toHaveLength(1);
    expect(isMultiNode()).toBe(true);
    expect(JSON.parse(store.get("sepia:nodes") ?? "[]")).toHaveLength(1);

    removePeer("node_1");
    expect(getPeers()).toEqual([]);
    expect(isMultiNode()).toBe(false);
    expect(JSON.parse(store.get("sepia:nodes") ?? "[]")).toEqual([]);
  });

  it("a blank or missing secret links no credential", async () => {
    mockedGetNode.mockResolvedValue({
      id: "n",
      name: "n",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    expect((await addPeer("http://h", { secret: "   " })).credentialId).toBeUndefined();
    mockedGetNode.mockResolvedValue({
      id: "n2",
      name: "n2",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    expect((await addPeer("http://h2", null)).credentialId).toBeUndefined();
    // A dangling credential id (deleted between pick and submit) links nothing.
    mockedGetNode.mockResolvedValue({
      id: "n3",
      name: "n3",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    const added = await addPeer("http://h3", { credentialId: "cred_gone" });
    expect(added.credentialId).toBeUndefined();
    expect(mockedGetNode).toHaveBeenLastCalledWith({
      baseUrl: "http://h3",
      token: null,
      timeoutMs: 5_000,
    });
  });

  it("addPeer links an existing credential and probes with its secret", async () => {
    const shared = addCredential({ label: "shared", secret: "shared-secret" });
    mockedGetNode.mockResolvedValue({
      id: "n",
      name: "n",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    const added = await addPeer("http://h", { credentialId: shared.id });
    expect(added.credentialId).toBe(shared.id);
    expect(mockedGetNode).toHaveBeenCalledWith({
      baseUrl: "http://h",
      token: "shared-secret",
      timeoutMs: 5_000,
    });
    // The store gains no duplicate credential for a relink.
    expect(credentialsStore.state).toEqual([shared]);
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
      credentialId: expect.any(String),
    });
    // The issued token files into the credential store under the node's name.
    const credential = credentialById(added.credentialId);
    expect(credential?.secret).toBe("sepia_issued");
    expect(credential?.label).toBe("thinkpad");
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
      selfStatus: "unknown",
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
    await expect(addPeer("http://x", null)).rejects.toThrow("already in the list");
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
    expect(nodesStore.state.selfStatus).toBe("online");
  });

  it("refreshSelf failure clears self and marks the node offline", async () => {
    mockedGetNode.mockRejectedValue(new Error("down"));
    await expect(refreshSelf()).rejects.toThrow("down");
    expect(nodesStore.state.self).toBeNull();
    expect(nodesStore.state.selfStatus).toBe("offline");
  });

  it("refreshSelf on 401 rethrows without marking the node offline", async () => {
    mockedGetNode.mockRejectedValue(
      Object.assign(new Error("Unauthorized"), { name: "AuthError" }),
    );
    await expect(refreshSelf()).rejects.toThrow("Unauthorized");
    expect(nodesStore.state.selfStatus).toBe("unknown");
  });
});

describe("nodeTarget / nodeName", () => {
  it("resolves local, peer, and unknown node ids", () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
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
    vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
    expect(nodeName(undefined)).toBe("this machine");
    nodesStore.setState(() => ({
      selfStatus: "unknown",
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

  it("nodeName falls back to 'local' when accessed remotely", () => {
    // No `location` (node env → isLocalAccess false) — a remote device must
    // not see "this machine".
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [] }));
    expect(nodeName(undefined)).toBe("local");
  });

  it("peer alias overrides the self-reported name", () => {
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [peer("node_p", "Thinkpad")],
    }));
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

describe("isThisMachine", () => {
  it("treats loopback hosts as this machine, wherever the UI is served", () => {
    for (const url of [
      "http://localhost:8787",
      "https://localhost",
      "http://127.0.0.1:8787",
      // The whole 127/8 block is loopback, not just 127.0.0.1.
      "http://127.1.2.3:8787",
      "http://[::1]:8787",
      // RFC 6761 — *.localhost resolves loopback.
      "http://peer.localhost:8787",
    ]) {
      expect(isThisMachine(url)).toBe(true);
    }
  });

  it("LAN and remote hosts are not this machine; unparseable input fails safe", () => {
    vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
    for (const url of [
      "http://192.168.1.10:8787",
      "https://thinkpad.example",
      "http://10.0.0.5:8787",
    ]) {
      expect(isThisMachine(url)).toBe(false);
    }
    expect(isThisMachine("not a url")).toBe(false);
    expect(isThisMachine("")).toBe(false);
  });

  it("the serving origin counts only under local access", () => {
    // Browser on this machine — a node at exactly the UI's origin qualifies.
    vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
    expect(isThisMachine("http://localhost:3000")).toBe(true);

    // Browser on a phone, UI served by a laptop over LAN: the origin is NOT
    // this machine (isLocalAccess gates the origin clause), but a peer at
    // localhost is — loopback follows the browser's device, not the origin.
    vi.stubGlobal("location", { hostname: "192.168.1.5", origin: "http://192.168.1.5:3000" });
    expect(isThisMachine("http://192.168.1.5:3000")).toBe(false);
    expect(isThisMachine("http://localhost:8787")).toBe(true);
  });
});

describe("local node address override", () => {
  it("defaults to the relative same-origin target", () => {
    expect(localTarget()).toEqual({ baseUrl: "", token: getToken() });
    expect(nodeTarget(undefined).baseUrl).toBe("");
    expect(nodeTarget("local").baseUrl).toBe("");
  });

  it("localNodeAddress falls back to the serving origin, then follows the override", () => {
    vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
    expect(localNodeAddress()).toBe("http://localhost:3000");
    setLocalNodeUrl("http://thinkpad:8787");
    expect(localNodeAddress()).toBe("http://thinkpad:8787");
  });

  it("localTarget/nodeTarget resolve the override's absolute origin + local token", () => {
    setLocalNodeUrl("http://thinkpad:8787");
    expect(localTarget()).toEqual({ baseUrl: "http://thinkpad:8787", token: getToken() });
    expect(nodeTarget(undefined).baseUrl).toBe("http://thinkpad:8787");
    expect(nodeTarget("local").baseUrl).toBe("http://thinkpad:8787");
    // Peers resolve untouched — the override only moves the local leg.
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    expect(nodeTarget("node_p").baseUrl).toBe("https://node_p.example");
  });

  it("persists like every other pref, and clearing restores the default", () => {
    setLocalNodeUrl("https://node.example");
    expect(
      (JSON.parse(store.get("sepia:settings") ?? "{}") as { localNodeUrl?: string }).localNodeUrl,
    ).toBe("https://node.example");
    setLocalNodeUrl(null);
    expect(settingsStore.state.localNodeUrl).toBeNull();
    expect(localTarget().baseUrl).toBe("");
  });

  it("parse → save round-trip: a typed address canonicalizes into the pref", () => {
    const parsed = parseNodeAddress("  thinkpad  ");
    if (!parsed.ok) throw new Error(`expected accept, got: ${parsed.error}`);
    setLocalNodeUrl(parsed.address.url);
    expect(settingsStore.state.localNodeUrl).toBe("http://thinkpad:8787");
    expect(nodeTarget("local").baseUrl).toBe("http://thinkpad:8787");
  });

  it("fan-out's local leg and refreshSelf follow the override", async () => {
    setLocalNodeUrl("http://thinkpad:8787");
    mockedListSessions.mockResolvedValue([session("local-1")]);
    await listAllSessions();
    expect(mockedListSessions.mock.calls[0]?.[0]?.baseUrl).toBe("http://thinkpad:8787");

    mockedGetNode.mockResolvedValue({
      id: "node_thinkpad",
      name: "thinkpad",
      version: "1",
      protocol: 1,
      agents: [],
      capabilities: [],
    });
    await refreshSelf();
    expect(mockedGetNode).toHaveBeenCalledWith({
      baseUrl: "http://thinkpad:8787",
      token: getToken(),
    });
    expect(nodesStore.state.self?.id).toBe("node_thinkpad");
  });

  it("the name fallback follows the override's address, not the origin", () => {
    // Override pointing at a remote host — "this machine" would be a lie
    // even though the UI itself is loopback-served.
    vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
    setLocalNodeUrl("http://thinkpad:8787");
    expect(nodeName(undefined)).toBe("local");
    // …and a loopback override earns it back.
    setLocalNodeUrl("http://localhost:8787");
    expect(nodeName(undefined)).toBe("this machine");
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
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1", ["p1"])] : [session("peer-1", ["p2"])],
    );
    const rows = await listAllSessions();
    expect(rows.map((r) => `${r.node}:${r.id}`)).toEqual(["local:local-1", "node_p:peer-1"]);
    expect(rows[0]?.projectIds).toEqual(["local:p1"]);
    expect(rows[1]?.projectIds).toEqual(["node_p:p2"]);
  });

  it("listAllSessions requests lock state on every node and keeps it through tagging", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    mockedListSessions.mockResolvedValue([{ ...session("s1"), locked: true, lockHolderPid: 4242 }]);
    const rows = await listAllSessions();
    expect(mockedListSessions).toHaveBeenCalledTimes(2);
    for (const call of mockedListSessions.mock.calls) {
      expect(call[1]).toEqual({ withLocks: true });
    }
    // Lock fields ride the merge untouched on both local and peer rows.
    expect(rows.every((row) => row.locked && row.lockHolderPid === 4242)).toBe(true);
  });

  it("a dead peer contributes nothing; a dead local node degrades to offline", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1")] : Promise.reject(new Error("peer down")),
    );
    const rows = await listAllSessions();
    expect(rows).toHaveLength(1);
    expect(rows[0]?.id).toBe("local-1");
    expect(nodesStore.state.selfStatus).toBe("online");

    // Local degrades like a peer: its rows drop out, peers keep listing,
    // and the store records the machine as offline.
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? Promise.reject(new Error("local down")) : [session("peer-1")],
    );
    const degraded = await listAllSessions();
    expect(degraded.map((r) => `${r.node}:${r.id}`)).toEqual(["node_p:peer-1"]);
    expect(nodesStore.state.selfStatus).toBe("offline");

    // A later success flips it back — no restart needed.
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-2")] : [session("peer-1")],
    );
    const recovered = await listAllSessions();
    expect(recovered.map((r) => r.id)).toEqual(["local-2", "peer-1"]);
    expect(nodesStore.state.selfStatus).toBe("online");
  });

  it("listAllSessions resolves empty (not an error) when local is down with no peers", async () => {
    mockedListSessions.mockRejectedValue(new Error("local down"));
    await expect(listAllSessions()).resolves.toEqual([]);
    expect(nodesStore.state.selfStatus).toBe("offline");
  });

  it("a local 401 still throws — the TokenGate path, not degradation", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === ""
        ? Promise.reject(Object.assign(new Error("Unauthorized"), { name: "AuthError" }))
        : [session("peer-1")],
    );
    await expect(listAllSessions()).rejects.toThrow("Unauthorized");
    // Reachable-but-unauthorized isn't an outage — the marker doesn't move.
    expect(nodesStore.state.selfStatus).toBe("unknown");
  });

  it("listAllProjects and listAllAgents degrade the local leg the same way", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    mockedListProjects.mockImplementation(async (target) =>
      target?.baseUrl === ""
        ? Promise.reject(new Error("local down"))
        : { projects: [{ id: "p2", name: "p2" }] },
    );
    expect(await listAllProjects()).toEqual([{ id: "p2", name: "p2", node: "node_p" }]);
    expect(nodesStore.state.selfStatus).toBe("offline");

    mockedListAgents.mockImplementation(async (target) =>
      target?.baseUrl === ""
        ? Promise.reject(new Error("local down"))
        : [{ id: "devin", label: "Devin peer" }],
    );
    expect(await listAllAgents()).toEqual([{ id: "devin", label: "Devin peer" }]);
  });

  it("listAllProjects tags rows by node and tolerates peer failure", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
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
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [peer("node_p"), peer("node_dead")],
    }));
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
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [gatewayPeer(), peer("node_direct")],
    }));
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

  it("normalizePeer passes credentialId through and ignores the legacy inline token", () => {
    expect(normalizePeer({ id: "n", url: "http://h", credentialId: "cred_1" })?.credentialId).toBe(
      "cred_1",
    );
    // `token` doesn't read onto the peer — loadPeers upgrades it into the
    // credential store (see credentials.test.ts).
    expect(normalizePeer({ id: "n", url: "http://h", token: "t" })?.credentialId).toBeUndefined();
    expect(
      normalizePeer({ id: "n", url: "http://h", credentialId: "" })?.credentialId,
    ).toBeUndefined();
    expect(
      normalizePeer({ id: "n", url: "http://h", credentialId: 5 })?.credentialId,
    ).toBeUndefined();
  });

  it("peerTarget resolves the linked credential's secret and fails safe on a dangling link", () => {
    const linked = peer("node_p");
    expect(peerTarget(linked).token).toBe("tok-node_p");
    // The credential was deleted out from under the link — no auth, no stale secret.
    expect(peerTarget({ ...linked, credentialId: "cred_gone" }).token).toBeNull();
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
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [gatewayPeer(), peer("node_direct")],
    }));
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
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gatewayPeer()] }));
    mockedDeleteServer.mockRejectedValue(new Error("Unknown server"));

    await removePeerEntry("node_remote");
    expect(getPeers()).toEqual([]);
  });
});

describe("peer address helpers", () => {
  it("peerUrlParts splits scheme/host/port and fills default ports", () => {
    expect(peerUrlParts("https://peer.example")).toEqual({
      scheme: "https",
      host: "peer.example",
      port: 443,
    });
    expect(peerUrlParts("http://peer.example")).toEqual({
      scheme: "http",
      host: "peer.example",
      port: 80,
    });
    expect(peerUrlParts("http://peer.example:8787")).toEqual({
      scheme: "http",
      host: "peer.example",
      port: 8787,
    });
  });

  // Narrows the ok/error union — a reject fails the test with the message.
  const parseOk = (input: string) => {
    const parsed = parseNodeAddress(input);
    if (!parsed.ok) throw new Error(`expected accept, got: ${parsed.error}`);
    return parsed.address;
  };

  it("parseNodeAddress accepts a bare host and defaults to http + the sepia port", () => {
    expect(parseOk("thinkpad")).toEqual({
      url: "http://thinkpad:8787",
      scheme: "http",
      host: "thinkpad",
      port: 8787,
    });
    expect(parseOk("  192.168.1.10  ")).toEqual({
      url: "http://192.168.1.10:8787",
      scheme: "http",
      host: "192.168.1.10",
      port: 8787,
    });
  });

  it("parseNodeAddress accepts host:port and full http(s) addresses", () => {
    expect(parseOk("thinkpad:9999")).toMatchObject({ url: "http://thinkpad:9999", port: 9999 });
    // http with no :port lands on the sepia serve default, not 80.
    expect(parseOk("http://peer.example")).toMatchObject({
      url: "http://peer.example:8787",
      scheme: "http",
      port: 8787,
    });
    expect(parseOk("https://peer.example")).toMatchObject({
      url: "https://peer.example",
      scheme: "https",
      port: 443,
    });
    expect(parseOk("https://peer.example:8443")).toMatchObject({
      url: "https://peer.example:8443",
      port: 8443,
    });
    // IPv6 keeps its brackets; uppercase schemes normalize.
    expect(parseOk("http://[::1]:8787").url).toBe("http://[::1]:8787");
    expect(parseOk("HTTPS://Peer.Example").url).toBe("https://peer.example");
  });

  it("parseNodeAddress drops a pasted URL's path and elides URL-default ports", () => {
    expect(parseOk("https://peer.example:8443/api?q=1").url).toBe("https://peer.example:8443");
    expect(parseOk("http://peer.example:80").url).toBe("http://peer.example");
    expect(parseOk("https://peer.example:443").url).toBe("https://peer.example");
  });

  it("parseNodeAddress treats a dangling colon as an empty port → scheme default", () => {
    expect(parseOk("peer.example:")).toMatchObject({ url: "http://peer.example:8787", port: 8787 });
    expect(parseOk("https://peer.example:")).toMatchObject({
      url: "https://peer.example",
      port: 443,
    });
  });

  it("parseNodeAddress rejects non-http(s) schemes", () => {
    for (const input of ["wss://peer.example", "ftp://peer.example", "ws://peer.example:8787"]) {
      expect(parseNodeAddress(input)).toEqual({
        ok: false,
        error: "Only http:// and https:// addresses are supported",
      });
    }
  });

  it("parseNodeAddress rejects empty, unparseable and out-of-range input", () => {
    expect(parseNodeAddress("")).toEqual({ ok: false, error: "Address is required" });
    expect(parseNodeAddress("   ")).toEqual({ ok: false, error: "Address is required" });
    expect(parseNodeAddress("http://")).toEqual({ ok: false, error: "Host is required" });
    expect(parseNodeAddress("bad host")).toEqual({
      ok: false,
      error: "Enter a hostname or an http(s) address",
    });
    expect(parseNodeAddress("host:notaport").ok).toBe(false);
    expect(parseNodeAddress("host:99999").ok).toBe(false);
    expect(parseNodeAddress("host:0")).toEqual({
      ok: false,
      error: "Port must be a number from 1 to 65535",
    });
  });
});

describe("enabled peers", () => {
  const disabled = (id: string): PeerNode => ({ ...peer(id), enabled: false });

  it("normalizePeer treats absent as enabled and keeps an explicit false", () => {
    expect(normalizePeer(peer("a"))?.enabled).toBeUndefined();
    expect(normalizePeer({ ...peer("a"), enabled: false })?.enabled).toBe(false);
    // Non-boolean legacy values normalize away — the peer stays enabled.
    expect(normalizePeer({ ...peer("a"), enabled: "no" })?.enabled).toBeUndefined();
    expect(isPeerEnabled(normalizePeer(peer("a"))!)).toBe(true);
  });

  it("setPeerEnabled parks and un-parks a peer, persisting the flag", () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    setPeerEnabled("a", false);
    expect(nodesStore.state.peers[0]?.enabled).toBe(false);
    expect((JSON.parse(store.get("sepia:nodes") ?? "[]") as PeerNode[])[0]?.enabled).toBe(false);
    setPeerEnabled("a", true);
    expect(nodesStore.state.peers[0]?.enabled).toBeUndefined();
    expect(isPeerEnabled(nodesStore.state.peers[0]!)).toBe(true);
  });

  it("isMultiNode counts only enabled peers", () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [disabled("a")] }));
    expect(isMultiNode()).toBe(false);
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [disabled("a"), peer("b")],
    }));
    expect(isMultiNode()).toBe(true);
  });

  it("peerTarget/nodeTarget resolve disabled peers to the unreachable sentinel", () => {
    const target = peerTarget(disabled("node_off"));
    expect(target.baseUrl).toBe("http://node_off.invalid");
    expect(target.token).toBeNull();
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [disabled("node_off")],
    }));
    expect(nodeTarget("node_off").baseUrl).toBe("http://node_off.invalid");
    // Display names still resolve — a parked peer keeps its identity.
    expect(nodeName("node_off")).toBe("node_off");
  });

  it("disabled peers are skipped by the fan-out lists", async () => {
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [disabled("node_off"), peer("node_on")],
    }));
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1")] : [session(`peer-${target?.baseUrl}`)],
    );
    const rows = await listAllSessions();
    const calledUrls = mockedListSessions.mock.calls.map(([target]) => target?.baseUrl);
    expect(calledUrls).not.toContain("https://node_off.example");
    expect(rows.map((r) => `${r.node}:${r.id}`)).toEqual([
      "local:local-1",
      "node_on:peer-https://node_on.example",
    ]);
  });

  it("with every peer disabled, local rows stay untagged (single-node shape)", async () => {
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [disabled("node_off")],
    }));
    mockedListSessions.mockResolvedValue([session("local-1", ["p1"])]);
    const rows = await listAllSessions();
    expect(rows[0]?.node).toBeUndefined();
    expect(rows[0]?.projectIds).toEqual(["p1"]);
    expect(mockedListSessions).toHaveBeenCalledTimes(1);
  });

  it("listAllProjects and listAllAgents skip disabled peers", async () => {
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [disabled("node_off"), peer("node_on")],
    }));
    mockedListProjects.mockImplementation(async (target) =>
      target?.baseUrl === "" ? { projects: [{ id: "p1", name: "p1" }] } : { projects: [] },
    );
    await listAllProjects();
    expect(mockedListProjects.mock.calls.map(([target]) => target?.baseUrl)).not.toContain(
      "https://node_off.example",
    );

    mockedListAgents.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [{ id: "devin", label: "Devin" }] : [],
    );
    await listAllAgents();
    expect(mockedListAgents.mock.calls.map(([target]) => target?.baseUrl)).not.toContain(
      "https://node_off.example",
    );
  });
});

describe("disabled local node", () => {
  it("isLocalNodeEnabled follows the settings pref and persists it", () => {
    expect(isLocalNodeEnabled()).toBe(true);
    setLocalNodeEnabled(false);
    expect(isLocalNodeEnabled()).toBe(false);
    expect(settingsStore.state.localNodeEnabled).toBe(false);
    // The flag persists like every other settings pref.
    expect(
      (JSON.parse(store.get("sepia:settings") ?? "{}") as { localNodeEnabled?: boolean })
        .localNodeEnabled,
    ).toBe(false);
    setLocalNodeEnabled(true);
    expect(isLocalNodeEnabled()).toBe(true);
  });

  it("a disabled local runs no fan-out leg — peers keep listing untouched", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    setLocalNodeEnabled(false);
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1")] : [session("peer-1")],
    );
    const rows = await listAllSessions();
    // The local origin was never called — parked means no leg at all.
    expect(mockedListSessions.mock.calls.map(([target]) => target?.baseUrl)).toEqual([
      "https://node_p.example",
    ]);
    expect(rows).toEqual([expect.objectContaining({ id: "peer-1", node: "node_p" })]);
    // The skipped leg doesn't move the local reachability marker — that's
    // refreshSelf's job (the serving node may still be perfectly reachable).
    expect(nodesStore.state.selfStatus).toBe("unknown");
  });

  it("a disabled local contributes nothing even with no peers", async () => {
    setLocalNodeEnabled(false);
    mockedListSessions.mockResolvedValue([session("local-1")]);
    expect(await listAllSessions()).toEqual([]);
    expect(mockedListSessions).not.toHaveBeenCalled();
    expect(nodesStore.state.selfStatus).toBe("unknown");
  });

  it("listAllProjects and listAllAgents skip the local leg the same way", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    setLocalNodeEnabled(false);
    mockedListProjects.mockImplementation(async (target) =>
      target?.baseUrl === "" ? { projects: [{ id: "p1", name: "p1" }] } : { projects: [] },
    );
    await listAllProjects();
    expect(mockedListProjects.mock.calls.map(([target]) => target?.baseUrl)).toEqual([
      "https://node_p.example",
    ]);

    mockedListAgents.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [{ id: "devin", label: "Devin" }] : [],
    );
    await listAllAgents();
    expect(mockedListAgents.mock.calls.map(([target]) => target?.baseUrl)).toEqual([
      "https://node_p.example",
    ]);
  });

  it("nodeTarget still resolves local — the origin is the transport, not a data source", () => {
    setLocalNodeEnabled(false);
    expect(nodeTarget(undefined)).toEqual({ baseUrl: "", token: getToken() });
    expect(nodeTarget("local")).toEqual({ baseUrl: "", token: getToken() });
  });

  it("isMultiNode ignores the local flag — only enabled peers count", () => {
    setLocalNodeEnabled(false);
    expect(isMultiNode()).toBe(false);
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("b")] }));
    expect(isMultiNode()).toBe(true);
  });

  it("re-enabling restores the local leg on the next fetch", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("node_p")] }));
    setLocalNodeEnabled(false);
    mockedListSessions.mockImplementation(async (target) =>
      target?.baseUrl === "" ? [session("local-1")] : [session("peer-1")],
    );
    expect((await listAllSessions()).map((r) => r.id)).toEqual(["peer-1"]);
    setLocalNodeEnabled(true);
    expect((await listAllSessions()).map((r) => `${r.node}:${r.id}`)).toEqual([
      "local:local-1",
      "node_p:peer-1",
    ]);
  });
});

describe("updatePeerEntry", () => {
  it("updates a direct peer's url and files a fresh secret as a credential", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    const before = getPeers()[0];
    await updatePeerEntry("a", {
      url: "https://new.example:9000",
      credential: { secret: "fresh" },
    });
    const updated = getPeers()[0];
    expect(updated?.url).toBe("https://new.example:9000");
    expect(peerSecret(updated!)).toBe("fresh");
    // A new secret files a NEW credential — the old link is replaced.
    expect(updated?.credentialId).not.toBe(before?.credentialId);
    expect(credentialById(updated?.credentialId)?.label).toBe("a");
    expect(mockedListServers).not.toHaveBeenCalled();
    expect(mockedUpdateServer).not.toHaveBeenCalled();
  });

  it("keeps the link by default, clears on null, relinks a stored credential", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    const original = getPeers()[0]?.credentialId;
    await updatePeerEntry("a", {});
    expect(getPeers()[0]?.credentialId).toBe(original);
    await updatePeerEntry("a", { credential: null });
    expect(getPeers()[0]?.credentialId).toBeUndefined();
    expect(peerSecret(getPeers()[0]!)).toBeNull();

    const shared = addCredential({ label: "shared", secret: "s2" });
    await updatePeerEntry("a", { credential: { credentialId: shared.id } });
    expect(getPeers()[0]?.credentialId).toBe(shared.id);
    expect(peerTarget(getPeers()[0]!).token).toBe("s2");
  });

  it("routes a gateway peer's edits through updateServer, preserving the stored credential", async () => {
    const managed = {
      id: "srv_1",
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http" as const,
      auth: { type: "token" as const, secret: SECRET_MASK },
      ssh: { host: "bastion", port: 22, user: "ops", key: "/keys/id" },
    };
    const gw: PeerNode = {
      id: "node_gw",
      name: "gw",
      url: "http://remote.example:8787",
      via: "gateway",
      serverId: "srv_1",
    };
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gw] }));
    mockedListServers.mockResolvedValue([managed]);
    mockedUpdateServer.mockResolvedValue(managed);

    await updatePeerEntry("node_gw", { url: "https://remote2.example", token: SECRET_MASK });
    expect(mockedUpdateServer).toHaveBeenCalledWith("srv_1", {
      label: "remote.example",
      host: "remote2.example",
      port: 443,
      scheme: "https",
      // Masked auth + ssh round-trip untouched — the server keeps the stored secret.
      auth: { type: "token", user: undefined, secret: SECRET_MASK },
      ssh: { host: "bastion", port: 22, user: "ops", key: "/keys/id" },
    });
    // The peer record keeps no credential link — the browser never holds it.
    expect(getPeers()[0]?.url).toBe("https://remote2.example");
    expect(getPeers()[0]?.credentialId).toBeUndefined();
  });

  it("a new token on a gateway peer replaces the managed credential; empty clears it", async () => {
    const managed = {
      id: "srv_1",
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http" as const,
      auth: { type: "token" as const, secret: SECRET_MASK },
      ssh: null,
    };
    const gw: PeerNode = {
      id: "node_gw",
      name: "gw",
      url: "http://remote.example:8787",
      via: "gateway",
      serverId: "srv_1",
    };
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gw] }));
    mockedListServers.mockResolvedValue([managed]);
    mockedUpdateServer.mockResolvedValue(managed);

    await updatePeerEntry("node_gw", { token: "fresh-secret" });
    expect(mockedUpdateServer.mock.calls[0]?.[1].auth).toEqual({
      type: "token",
      secret: "fresh-secret",
    });
    await updatePeerEntry("node_gw", { token: "" });
    expect(mockedUpdateServer.mock.calls[1]?.[1].auth).toBeNull();
  });

  it("an ssh object replaces the managed entry's tunnel; null clears it", async () => {
    const managed = {
      id: "srv_1",
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http" as const,
      auth: { type: "token" as const, secret: SECRET_MASK },
      ssh: { host: "bastion", port: 22, user: "ops", key: "/keys/id" },
    };
    const gw: PeerNode = {
      id: "node_gw",
      name: "gw",
      url: "http://remote.example:8787",
      via: "gateway",
      serverId: "srv_1",
    };
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gw] }));
    mockedListServers.mockResolvedValue([managed]);
    mockedUpdateServer.mockResolvedValue(managed);

    await updatePeerEntry("node_gw", {
      ssh: { host: "jump.example", port: 2222, user: "deploy" },
    });
    expect(mockedUpdateServer.mock.calls[0]?.[1].ssh).toEqual({
      host: "jump.example",
      port: 2222,
      user: "deploy",
    });
    // url/auth untouched — the submitted ssh is the only change.
    expect(mockedUpdateServer.mock.calls[0]?.[1].host).toBe("remote.example");
    expect(mockedUpdateServer.mock.calls[0]?.[1].auth).toEqual({
      type: "token",
      user: undefined,
      secret: SECRET_MASK,
    });

    await updatePeerEntry("node_gw", { ssh: null });
    expect(mockedUpdateServer.mock.calls[1]?.[1].ssh).toBeNull();
  });

  it("a masked ssh key passes through — the server keeps the stored material", async () => {
    const managed = {
      id: "srv_1",
      label: "remote.example",
      host: "remote.example",
      port: 8787,
      scheme: "http" as const,
      auth: null,
      // GET masks an inline PEM — the form echoes it back untouched.
      ssh: { host: "bastion", port: 22, user: "ops", key: SECRET_MASK },
    };
    const gw: PeerNode = {
      id: "node_gw",
      name: "gw",
      url: "http://remote.example:8787",
      via: "gateway",
      serverId: "srv_1",
    };
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gw] }));
    mockedListServers.mockResolvedValue([managed]);
    mockedUpdateServer.mockResolvedValue(managed);

    await updatePeerEntry("node_gw", {
      ssh: { host: "bastion", port: 22, user: "ops", key: SECRET_MASK },
    });
    expect(mockedUpdateServer).toHaveBeenCalledWith(
      "srv_1",
      expect.objectContaining({
        ssh: { host: "bastion", port: 22, user: "ops", key: SECRET_MASK },
      }),
    );
  });

  it("a gateway peer whose managed entry is gone recreates it with the new url", async () => {
    const gw: PeerNode = {
      id: "node_gw",
      name: "gw",
      url: "http://remote.example:8787",
      via: "gateway",
      serverId: "srv_gone",
    };
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gw] }));
    mockedListServers.mockResolvedValue([]);
    mockedCreateServer.mockResolvedValue({
      id: "srv_recreated",
      label: "gw",
      host: "remote.example",
      port: 9999,
      scheme: "http" as const,
      auth: null,
      ssh: null,
    });

    // `via` absent keeps gateway routing — and since the entry must exist
    // for the routing to work, a vanished one is recreated rather than
    // leaving a dead gateway hop.
    await updatePeerEntry("node_gw", { url: "http://remote.example:9999" });
    expect(mockedUpdateServer).not.toHaveBeenCalled();
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "gw",
      host: "remote.example",
      port: 9999,
      scheme: "http",
      auth: null,
      ssh: null,
    });
    expect(getPeers()[0]?.url).toBe("http://remote.example:9999");
    expect(getPeers()[0]?.serverId).toBe("srv_recreated");
    expect(getPeers()[0]?.via).toBe("gateway");
  });

  it("an unknown peer id is a no-op", async () => {
    await expect(updatePeerEntry("ghost", { url: "http://x" })).resolves.toBeUndefined();
  });
});

describe("updatePeerEntry routing transitions", () => {
  const gwPeer = (): PeerNode => ({
    id: "node_gw",
    name: "gw",
    url: "http://remote.example:8787",
    via: "gateway",
    serverId: "srv_1",
  });
  const managed = {
    id: "srv_1",
    label: "remote.example",
    host: "remote.example",
    port: 8787,
    scheme: "http" as const,
    auth: { type: "token" as const, secret: SECRET_MASK },
    ssh: null,
  };

  it("direct → gateway creates a managed entry carrying the linked credential", async () => {
    nodesStore.setState(() => ({
      self: null,
      selfStatus: "unknown",
      peers: [{ ...peer("a"), alias: "work laptop" }],
    }));
    mockedCreateServer.mockResolvedValue({ ...managed, id: "srv_new" });

    await updatePeerEntry("a", { via: "gateway", token: SECRET_MASK });
    // The kept (masked) credential moves from the browser into the node's
    // store; the label comes from the peer's display name.
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "work laptop",
      host: "a.example",
      port: 443,
      scheme: "https",
      auth: { type: "token", secret: "tok-a" },
      ssh: null,
    });
    const updated = getPeers()[0];
    expect(updated?.via).toBe("gateway");
    expect(updated?.serverId).toBe("srv_new");
    // The browser link is cleared — the credential now lives server-side
    // (the credential record itself stays in the store for other peers).
    expect(updated?.credentialId).toBeUndefined();
    expect(peerTarget(updated!)).toEqual({
      baseUrl: "/api/gateway/srv_new",
      token: getToken(),
      timeoutMs: 12_000,
    });
    const persisted = JSON.parse(store.get("sepia:nodes") ?? "[]") as PeerNode[];
    expect(persisted[0]?.serverId).toBe("srv_new");
  });

  it("direct → gateway uses a freshly typed token, or stores no auth on empty", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    mockedCreateServer.mockResolvedValue(managed);

    await updatePeerEntry("a", { via: "gateway", token: "new-secret" });
    expect(mockedCreateServer.mock.calls[0]?.[0].auth).toEqual({
      type: "token",
      secret: "new-secret",
    });

    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("b")] }));
    await updatePeerEntry("b", { via: "gateway", token: "  " });
    expect(mockedCreateServer.mock.calls[1]?.[0].auth).toBeNull();
  });

  it("direct → gateway leaves the peer untouched when createServer fails", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    mockedCreateServer.mockRejectedValue(new Error("server is down"));

    await expect(updatePeerEntry("a", { via: "gateway" })).rejects.toThrow("server is down");
    expect(getPeers()[0]?.via).toBeUndefined();
    expect(peerSecret(getPeers()[0]!)).toBe("tok-a");
  });

  it("gateway → direct deletes the managed entry and keeps no credential when none submitted", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedDeleteServer.mockResolvedValue(undefined);

    await updatePeerEntry("node_gw", { via: "direct" });
    expect(mockedDeleteServer).toHaveBeenCalledWith("srv_1");
    const updated = getPeers()[0];
    expect(updated?.via).toBeUndefined();
    expect(updated?.serverId).toBeUndefined();
    // The managed secret can't come back — absent `credential` on the switch
    // links nothing, and calls go out unauthenticated.
    expect(updated?.credentialId).toBeUndefined();
    expect(peerTarget(updated!)).toEqual({
      baseUrl: "http://remote.example:8787",
      token: null,
      timeoutMs: 3_000,
    });
    // The persisted record carries neither gateway field.
    const persisted = JSON.parse(store.get("sepia:nodes") ?? "[]") as Record<string, unknown>[];
    expect("via" in persisted[0]!).toBe(false);
    expect("serverId" in persisted[0]!).toBe(false);
  });

  it("gateway → direct files a freshly typed secret as a credential for the browser to hold", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedDeleteServer.mockResolvedValue(undefined);

    await updatePeerEntry("node_gw", {
      via: "direct",
      credential: { secret: "browser-token" },
    });
    const updated = getPeers()[0];
    expect(peerSecret(updated!)).toBe("browser-token");
    expect(credentialById(updated?.credentialId)?.label).toBe("gw");
  });

  it("gateway → direct can relink a credential already in the store", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedDeleteServer.mockResolvedValue(undefined);
    const stored = addCredential({ label: "shared", secret: "s-shared" });

    await updatePeerEntry("node_gw", {
      via: "direct",
      credential: { credentialId: stored.id },
    });
    const updated = getPeers()[0];
    expect(updated?.credentialId).toBe(stored.id);
    expect(peerTarget(updated!).token).toBe("s-shared");
  });

  it("gateway → direct still flips when the managed delete fails", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedDeleteServer.mockRejectedValue(new Error("Unknown server"));

    await updatePeerEntry("node_gw", { via: "direct" });
    expect(getPeers()[0]?.via).toBeUndefined();
    expect(getPeers()[0]?.serverId).toBeUndefined();
  });

  it("staying gateway with a vanished managed entry recreates it", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedListServers.mockResolvedValue([]);
    mockedCreateServer.mockResolvedValue({ ...managed, id: "srv_repaired" });

    await updatePeerEntry("node_gw", { via: "gateway", token: "reissued" });
    expect(mockedUpdateServer).not.toHaveBeenCalled();
    expect(mockedCreateServer).toHaveBeenCalledWith({
      label: "gw",
      host: "remote.example",
      port: 8787,
      scheme: "http",
      auth: { type: "token", secret: "reissued" },
      ssh: null,
    });
    expect(getPeers()[0]?.serverId).toBe("srv_repaired");
    expect(getPeers()[0]?.via).toBe("gateway");
  });

  it("direct → gateway carries a submitted ssh config into the new entry", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("a")] }));
    mockedCreateServer.mockResolvedValue(managed);

    await updatePeerEntry("a", {
      via: "gateway",
      token: SECRET_MASK,
      ssh: { host: "bastion", port: 2222, user: "ops", key: "/keys/id" },
    });
    expect(mockedCreateServer).toHaveBeenCalledWith(
      expect.objectContaining({
        ssh: { host: "bastion", port: 2222, user: "ops", key: "/keys/id" },
      }),
    );

    // A masked key on the create path has no stored material to keep —
    // it's dropped rather than written as a literal.
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [peer("b")] }));
    await updatePeerEntry("b", {
      via: "gateway",
      token: SECRET_MASK,
      ssh: { host: "bastion", port: 22, user: "ops", key: SECRET_MASK },
    });
    expect(mockedCreateServer.mock.calls[1]?.[0].ssh).toEqual({
      host: "bastion",
      port: 22,
      user: "ops",
    });
  });

  it("staying gateway keeps PATCHing the entry when it still exists", async () => {
    nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [gwPeer()] }));
    mockedListServers.mockResolvedValue([managed]);
    mockedUpdateServer.mockResolvedValue(managed);

    await updatePeerEntry("node_gw", { via: "gateway", url: "https://remote2.example" });
    expect(mockedCreateServer).not.toHaveBeenCalled();
    expect(mockedDeleteServer).not.toHaveBeenCalled();
    expect(mockedUpdateServer).toHaveBeenCalledWith("srv_1", {
      label: "remote.example",
      host: "remote2.example",
      port: 443,
      scheme: "https",
      auth: { type: "token", user: undefined, secret: SECRET_MASK },
      ssh: null,
    });
    expect(getPeers()[0]?.via).toBe("gateway");
    expect(getPeers()[0]?.serverId).toBe("srv_1");
  });
});
