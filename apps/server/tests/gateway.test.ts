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

  it("refuses non-/api paths and dot-segment escapes", async () => {
    const { store, entry } = makeStore();
    let fetched = false;
    const fetchImpl: typeof fetch = () => {
      fetched = true;
      return Promise.resolve(new Response("ok"));
    };
    const deps = makeDeps(store, fetchImpl);

    const outside = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/admin`),
      [entry.id, "admin"],
      deps,
    );
    expect(outside?.status).toBe(400);

    // `%2e%2e` decodes to `..` — the check runs on the normalized URL, so
    // the escape resolves to /node before the /api prefix is required.
    const escape = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/%2e%2e/node`),
      [entry.id, "api", "%2e%2e", "node"],
      deps,
    );
    expect(escape?.status).toBe(400);

    expect(fetched).toBe(false);
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

describe("gateway hardening", () => {
  it("times out a peer that holds the socket without answering (504)", async () => {
    const { store, entry } = makeStore();
    const fetchImpl: typeof fetch = (_input, init) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(new DOMException("The operation timed out", "TimeoutError")),
        );
      });
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      { ...makeDeps(store, fetchImpl), timeoutMs: 20 },
    );
    expect(res?.status).toBe(504);
    expect((await res?.json()) as { error: string }).toEqual({
      error: "Timed out waiting for Thinkpad",
    });
  });

  it("SSE-intended requests never get the upstream deadline", async () => {
    const { store, entry } = makeStore();
    let seenSignal: AbortSignal | undefined;
    const fetchImpl: typeof fetch = (_input, init) => {
      seenSignal = init?.signal ?? undefined;
      return Promise.resolve(new Response("ok"));
    };
    const deps = { ...makeDeps(store, fetchImpl), timeoutMs: 20 };

    // Path-detected: the two AG-UI SSE routes.
    for (const segments of [
      [entry.id, "api", "events"],
      [entry.id, "api", "sessions", "s1", "stream"],
    ]) {
      const request = req(`/api/gateway/${segments.join("/")}`);
      await handleGatewayRoute(request, segments, deps);
      // request.signal itself is passed through — no AbortSignal.any wrap.
      expect(seenSignal).toBe(request.signal);
    }

    // Header-detected: a client asking for event-stream on any path.
    const acceptReq = req(`/api/gateway/${entry.id}/api/other`, {
      headers: { accept: "text/event-stream" },
    });
    await handleGatewayRoute(acceptReq, [entry.id, "api", "other"], deps);
    expect(seenSignal).toBe(acceptReq.signal);

    // A plain request gets a combined signal (not the raw request.signal).
    const plainReq = req(`/api/gateway/${entry.id}/api/node`);
    await handleGatewayRoute(plainReq, [entry.id, "api", "node"], deps);
    expect(seenSignal).not.toBe(plainReq.signal);
  });

  it("rejects an over-limit body with 413 before any upstream call", async () => {
    const { store, entry } = makeStore();
    let fetched = false;
    const fetchImpl: typeof fetch = () => {
      fetched = true;
      return Promise.resolve(new Response("ok"));
    };
    // 9 × 1MB chunks — over the 8MB cap; streamed so the reader-side limit
    // (not the content-length fast path) is what trips.
    const nineMeg = new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < 9; i += 1) controller.enqueue(new Uint8Array(1024 * 1024));
        controller.close();
      },
    });
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/sessions/s1/prompt`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: nineMeg,
        // Node/undici requires duplex for a streamed request body.
        duplex: "half",
      } as RequestInit),
      [entry.id, "api", "sessions", "s1", "prompt"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(413);
    expect(fetched).toBe(false);
  });

  it("rejects on a declared content-length over the cap without reading", async () => {
    const { store, entry } = makeStore();
    let fetched = false;
    const fetchImpl: typeof fetch = () => {
      fetched = true;
      return Promise.resolve(new Response("ok"));
    };
    const res = await handleGatewayRoute(
      new Request(`http://localhost/api/gateway/${entry.id}/api/node`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "content-length": String(9 * 1024 * 1024),
        },
        body: "x",
      }),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(413);
    expect(fetched).toBe(false);
  });

  it("never forwards the caller's Authorization — even when the entry has no auth", async () => {
    const { store, entry } = makeStore({ ...INPUT, auth: null });
    let seenAuth: string | null = "unset";
    const fetchImpl: typeof fetch = (_input, init) => {
      seenAuth = new Headers(init?.headers).get("authorization");
      return Promise.resolve(new Response("{}"));
    };
    await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`, {
        headers: { authorization: "Bearer caller-token" },
      }),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(seenAuth).toBeNull();
  });

  it("does not leak upstream hop-by-hop headers (set-cookie, location)", async () => {
    const { store, entry } = makeStore();
    const fetchImpl: typeof fetch = () =>
      Promise.resolve(
        new Response("ok", {
          status: 302,
          headers: {
            "content-type": "text/plain",
            "set-cookie": "peer_session=abc; HttpOnly",
            location: "http://192.168.1.10:8787/login",
          },
        }),
      );
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      makeDeps(store, fetchImpl),
    );
    expect(res?.status).toBe(302);
    expect(res?.headers.get("content-type")).toBe("text/plain");
    expect(res?.headers.get("set-cookie")).toBeNull();
    expect(res?.headers.get("location")).toBeNull();
  });

  it("an ssh peer whose tunnel can't come up answers 502, not a crash", async () => {
    const { store, entry } = makeStore(SSH_INPUT);
    const deps = makeDeps(store);
    const failing: ServersRouteDeps = {
      ...deps,
      tunnels: { ...deps.tunnels, ensure: () => Promise.reject(new Error("ssh died")) },
    };
    const res = await handleGatewayRoute(
      req(`/api/gateway/${entry.id}/api/node`),
      [entry.id, "api", "node"],
      failing,
    );
    expect(res?.status).toBe(502);
    expect((await res?.json()) as { error: string }).toEqual({ error: "ssh died" });
  });

  it("a path-traversal peer segment is a store miss (404), never URL input", async () => {
    const { store } = makeStore();
    const res = await handleGatewayRoute(
      req("/api/gateway/%2e%2e/api/node"),
      ["%2e%2e", "api", "node"],
      makeDeps(store),
    );
    expect(res?.status).toBe(404);
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

    // …and the query-token form counts, but only on the SSE routes
    // EventSource needs it for — a non-SSE path can't authenticate via URL.
    const sse = await app(req("/api/gateway/srv_unknown/api/events?access_token=node-token"));
    expect(sse.status).toBe(404); // unknown peer — auth passed, lookup failed
    const nonSse = await app(req("/api/gateway/srv_x/api/node?access_token=node-token"));
    expect(nonSse.status).toBe(401);
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
