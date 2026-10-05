import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { pullProject, pushProject } from "../lib/api";
import { addCredential, credentialsStore } from "../lib/credentials";
import { localEndpoint, peerEndpoint } from "../lib/transfer";
import { nodesStore, type PeerNode } from "../lib/nodes";
import { settingsStore } from "../lib/settings";
import { setToken } from "../lib/token";

/**
 * Project transfer client wiring — peer → {url, token} resolution and the
 * SSE progress reader behind pullProject/pushProject (fetch is stubbed).
 */
const store = new Map<string, string>();

beforeEach(() => {
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
  vi.stubGlobal("location", { hostname: "localhost", origin: "http://localhost:3000" });
  nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [] }));
  credentialsStore.setState(() => []);
  settingsStore.setState((prev) => ({ ...prev, localNodeUrl: null }));
  setToken(null);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const directPeer = (id: string): PeerNode => ({
  id,
  name: id,
  url: `https://${id}.example`,
  credentialId: addCredential({ label: id, secret: `tok-${id}` }).id,
});

describe("peerEndpoint", () => {
  it("resolves a direct peer to its origin + stored credential", () => {
    const peer = directPeer("thinkpad");
    expect(peerEndpoint(peer)).toEqual({
      url: "https://thinkpad.example",
      token: "tok-thinkpad",
    });
  });

  it("resolves a dangling credential to no auth", () => {
    const peer: PeerNode = { id: "x", name: "x", url: "https://x.example", credentialId: "gone" };
    expect(peerEndpoint(peer)).toEqual({ url: "https://x.example", token: null });
  });

  it("routes a gateway peer through this node's gateway mount with the local token", () => {
    setToken("local-secret");
    const peer: PeerNode = {
      id: "remote",
      name: "remote",
      url: "https://remote.example",
      via: "gateway",
      serverId: "srv_1",
    };
    expect(peerEndpoint(peer)).toEqual({
      url: "http://localhost:3000/api/gateway/srv_1",
      token: "local-secret",
    });
  });

  it("uses the localNodeUrl override as the gateway origin when set", () => {
    settingsStore.setState((prev) => ({ ...prev, localNodeUrl: "https://hub.example" }));
    const peer: PeerNode = {
      id: "remote",
      name: "remote",
      url: "https://remote.example",
      via: "gateway",
      serverId: "srv_2",
    };
    // The token is address-bound — one entered for "" doesn't follow the
    // override — so the endpoint resolves no credential here.
    expect(peerEndpoint(peer)).toEqual({
      url: "https://hub.example/api/gateway/srv_2",
      token: null,
    });
  });
});

describe("localEndpoint", () => {
  it("is this machine's origin + token — the push-back-to-us address", () => {
    setToken("mine");
    expect(localEndpoint()).toEqual({ url: "http://localhost:3000", token: "mine" });
  });
});

const sse = (frames: string): Response =>
  new Response(
    new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode(frames));
        controller.close();
      },
    }),
  );

describe("pullProject / pushProject", () => {
  it("streams frames and resolves with the done payload", async () => {
    const seen: Array<{ url: string; body: string; auth?: string }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: unknown, init?: RequestInit) => {
        seen.push({
          url:
            typeof input === "string"
              ? input
              : input instanceof URL
                ? input.href
                : (input as Request).url,
          body: typeof init?.body === "string" ? init.body : "",
          auth: (init?.headers as Record<string, string> | undefined)?.authorization,
        });
        return sse(
          'event: start\ndata: {"direction":"pull","project":"p1"}\n\n' +
            'event: session\ndata: {"index":1,"total":2,"id":"s1","title":"One"}\n\n' +
            'event: session\ndata: {"index":2,"total":2,"id":"s2","title":"Two"}\n\n' +
            'event: done\ndata: {"project":{"id":"p1","name":"P"},"imported":[{"id":"s1"},{"id":"s2"}],"skipped":[],"truncated":false}\n\n',
        );
      }),
    );
    const frames: string[] = [];
    const summary = await pullProject(
      { source: { url: "https://src.example", token: "src-tok" }, project: "p1" },
      (frame) => frames.push(frame.event),
      { baseUrl: "", token: "local-tok" },
    );
    expect(seen[0]?.url).toBe("/api/projects/pull");
    expect(seen[0]?.auth).toBe("Bearer local-tok");
    // The PEER's credential goes in the JSON body, not this request's header.
    expect(JSON.parse(seen[0]!.body)).toEqual({
      source: { url: "https://src.example", token: "src-tok" },
      project: "p1",
    });
    expect(frames).toEqual(["start", "session", "session", "done"]);
    expect(summary.imported).toHaveLength(2);
  });

  it("rejects on an error frame with the server's message", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => sse('event: error\ndata: {"error":"Source refused"}\n\n')),
    );
    await expect(
      pushProject("p1", { url: "https://dst.example", token: null }, undefined, {
        baseUrl: "",
        token: null,
      }),
    ).rejects.toThrow("Source refused");
  });

  it("rejects on a non-OK status before reading the stream", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () => new Response(JSON.stringify({ error: "Unknown project" }), { status: 404 }),
      ),
    );
    await expect(
      pullProject({ source: { url: "https://src.example" }, project: "nope" }),
    ).rejects.toMatchObject({ status: 404 });
  });
});
