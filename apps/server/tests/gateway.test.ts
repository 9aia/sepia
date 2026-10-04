import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import type { ControlPlaneService } from "sepia-session-control";
import { createApp } from "../src/app";
import { createServerStore, type ServerInput } from "../src/servers";
import { handleGatewayRoute, type ServersRouteDeps } from "../src/servers-routes";
import type { TunnelManager } from "../src/ssh";

const INPUT: ServerInput = {
  label: "Thinkpad",
  host: "192.168.1.10",
  port: 8787,
  scheme: "http",
  auth: { type: "token", secret: "peer-secret" },
  ssh: null,
};

const SSH_INPUT: ServerInput = {
  ...INPUT,
  ssh: { host: "bastion.example.com", port: 22, user: "luis", key: "~/.ssh/id_ed25519" },
};

const tmp = (): string => mkdtempSync(join(tmpdir(), "sepia-gateway-"));

const makeStore = (input: ServerInput = INPUT) => {
  const dir = tmp();
  const store = createServerStore(join(dir, "s.json"), join(dir, "k.key"), {});
  const entry = store.create(input);
  return { store, entry };
};

const makeDeps = (
  store: ReturnType<typeof createServerStore>,
  fetchImpl?: ServersRouteDeps["fetchImpl"],
): ServersRouteDeps => {
  const tunnels: TunnelManager = {
    ensure: () => Promise.resolve({ localPort: 44_001 }),
    localPort: () => 44_001,
    close: () => {},
    closeAll: () => {},
  };
  return { store, tunnels, cors: {}, fetchImpl };
};

const req = (path: string, init?: RequestInit): Request =>
  new Request(`http://localhost${path}`, init);

const urlOf = (input: Parameters<typeof fetch>[0]): string =>
  typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;

/** The gateway routes never touch the control plane — a cast stub suffices. */
const stubPlane = {} as ControlPlaneService;

describe("handleGatewayRoute", () => {
  it("forwards path + query upstream with the stored credential injected", async () => {
    const { store, entry } = makeStore();
    let seen: { url?: string; auth?: string | null } = {};
    const fetchImpl: typeof fetch = (input, init) => {
      seen = {
        url: urlOf(input),
        auth: new Headers(init?.headers).get("authorization"),
      };
      return Promise.resolve(
        new Response('{"id":"node_x"}', {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
    };

    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node?x=1`, {
        headers: { authorization: "Bearer caller-token" },
      }),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(seen.url).toBe("http://192.168.1.10:8787/api/node?x=1");
    expect(seen.auth).toBe("Bearer peer-secret");
  });

  it("forwards to an https upstream when the entry is TLS-terminated", async () => {
    const { store, entry } = makeStore({ ...INPUT, scheme: "https", port: 443 });
    let seenUrl = "";
    const fetchImpl: typeof fetch = (input) => {
      seenUrl = urlOf(input);
      return Promise.resolve(new Response("{}"));
    };
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(seenUrl).toBe("https://192.168.1.10:443/api/node");
  });

  it("strips the caller's ?access_token so the peer never sees UI credentials", async () => {
    const { store, entry } = makeStore();
    let seenUrl = "";
    const fetchImpl: typeof fetch = (input) => {
      seenUrl = urlOf(input);
      return Promise.resolve(new Response("ok"));
    };

    // The SSE form: EventSource clients authenticate to this node via query.
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/sessions/s1/stream?access_token=caller-token&agent=devin`),
      [entry.id, "api", "sessions", "s1", "stream"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(seenUrl).toBe("http://192.168.1.10:8787/api/sessions/s1/stream?agent=devin");
    expect(seenUrl).not.toContain("access_token");
  });

  it("forwards POST bodies and content-type", async () => {
    const { store, entry } = makeStore();
    let seen: { method?: string; type?: string | null; body?: string } = {};
    const fetchImpl: typeof fetch = async (_input, init) => {
      seen = {
        method: init?.method,
        type: new Headers(init?.headers).get("content-type"),
        body: init?.body === undefined ? undefined : await new Response(init.body).text(),
      };
      return new Response('{"ok":true}', { headers: { "content-type": "application/json" } });
    };

    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/sessions/s1/prompt`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ text: "hi" }),
      }),
      [entry.id, "api", "sessions", "s1", "prompt"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(seen).toEqual({
      method: "POST",
      type: "application/json",
      body: '{"text":"hi"}',
    });
  });

  it("streams the upstream body through (SSE pass-through)", async () => {
    const { store, entry } = makeStore();
    const upstream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode("event: heartbeat\ndata: {}\n\n"));
        controller.close();
      },
    });
    const fetchImpl: typeof fetch = () =>
      Promise.resolve(
        new Response(upstream, {
          status: 200,
          headers: { "content-type": "text/event-stream" },
        }),
      );

    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/events`),
      [entry.id, "api", "events"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(200);
    expect(res?.headers.get("content-type")).toBe("text/event-stream");
    expect(await res?.text()).toBe("event: heartbeat\ndata: {}\n\n");
  });

  it("returns 404 for an unknown peer and 400 for a missing path", async () => {
    const { store, entry } = makeStore();
    const deps = makeDeps(store);

    const unknown = await handleGatewayRoute(
      req("/api/gateway/srv_nope/api/node"),
      ["srv_nope", "api", "node"],
      deps,
    );
    expect(unknown?.status).toBe(404);

    const bare = await handleGatewayRoute(req(`/api/gateway/${entry.id}`), [entry.id], deps);
    expect(bare?.status).toBe(400);
  });

  it("returns 502 when the upstream is unreachable", async () => {
    const { store, entry } = makeStore();
    const fetchImpl: typeof fetch = () => Promise.reject(new Error("ECONNREFUSED"));
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(502);
  });

  it("routes ssh-configured peers through the tunnel's loopback port", async () => {
    const { store, entry } = makeStore(SSH_INPUT);
    let seenUrl = "";
    const fetchImpl: typeof fetch = (input) => {
      seenUrl = urlOf(input);
      return Promise.resolve(new Response("{}"));
    };
    await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(seenUrl).toBe("http://127.0.0.1:44001/api/node");
  });
});

describe("app /api/gateway integration", () => {
  const auth = { authorization: "Bearer node-token" };

  it("requires the node's own bearer like every /api route", async () => {
    const { store } = makeStore();
    const app = createApp(stubPlane, {
      token: "node-token",
      servers: store,
      tunnels: makeDeps(store).tunnels,
    });
    const denied = await app(req("/api/gateway/srv_x/api/node"));
    expect(denied.status).toBe(401);

    // …and the query-token form used by EventSource counts too.
    const allowed = await app(req("/api/gateway/srv_unknown/api/node?access_token=node-token"));
    expect(allowed.status).toBe(404); // unknown peer — auth passed, lookup failed
  });

  it("is 501 when the server registry isn't configured", async () => {
    const app = createApp(stubPlane, { token: "node-token" });
    const res = await app(req("/api/gateway/srv_x/api/node", { headers: auth }));
    expect(res.status).toBe(501);
  });

  it("404s for a peer that isn't in the managed registry", async () => {
    const { store } = makeStore();
    const app = createApp(stubPlane, {
      token: "node-token",
      servers: store,
      tunnels: makeDeps(store).tunnels,
    });
    const res = await app(req("/api/gateway/srv_ghost/api/node", { headers: auth }));
    expect(res.status).toBe(404);
  });
});
