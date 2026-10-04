import {
  authHeader,
  publicServer,
  validateServerInput,
  type ServerEntry,
  type ServerStore,
} from "./servers";
import type { TunnelManager } from "./ssh";

/**
 * /api/servers route tree — kept in its own module so app.ts only grows one
 * dispatch line. Besides CRUD it exposes:
 *
 *   POST   /api/servers/:id/tunnel   → ensure the SSH forward is up
 *   DELETE /api/servers/:id/tunnel   → drop it
 *   ANY    /api/servers/:id/proxy/*  → authenticated passthrough to the managed
 *                                      node (through the tunnel when ssh is
 *                                      configured — lazily started)
 *   ANY    /api/gateway/:id/*        → the same passthrough, mounted at the
 *                                      phase-3 gateway path (docs/protocol.md)
 *                                      — see handleGatewayRoute below
 *
 * The proxy is what keeps secrets server-side: the browser calls
 * `/api/servers/x/proxy/api/node` and this route injects the stored
 * Authorization header. The caller's own bearer token is never forwarded.
 */
export interface ServersRouteDeps {
  readonly store: ServerStore;
  readonly tunnels: TunnelManager;
  readonly cors: Record<string, string>;
  /** Injectable for tests. */
  readonly fetchImpl?: typeof fetch;
}

const json = (body: unknown, status: number, cors: Record<string, string>): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...cors },
  });

/** Direct URL, or the loopback end of the SSH forward when configured. */
const upstreamBase = async (entry: ServerEntry, deps: ServersRouteDeps): Promise<string> => {
  if (entry.ssh === null) return `http://${entry.host}:${entry.port}`;
  const { localPort } = await deps.tunnels.ensure(entry);
  return `http://127.0.0.1:${localPort}`;
};

const proxy = async (
  request: Request,
  entry: ServerEntry,
  path: string,
  deps: ServersRouteDeps,
): Promise<Response> => {
  const fetchImpl = deps.fetchImpl ?? fetch;
  const url = new URL(request.url);
  // EventSource can't set headers, so stream callers authenticate to this
  // node via `?access_token`. Strip it before forwarding — the peer must only
  // ever see its own stored credential, never the caller's.
  url.searchParams.delete("access_token");
  let base: string;
  try {
    base = await upstreamBase(entry, deps);
  } catch (error) {
    return json(
      { error: error instanceof Error ? error.message : "Tunnel unavailable" },
      502,
      deps.cors,
    );
  }
  const headers: Record<string, string> = {};
  const contentType = request.headers.get("content-type");
  if (contentType !== null) headers["content-type"] = contentType;
  const auth = authHeader(entry.auth);
  if (auth !== null) headers.authorization = auth;

  const hasBody = request.method !== "GET" && request.method !== "HEAD";
  let upstream: Response;
  try {
    upstream = await fetchImpl(`${base}${path}${url.search}`, {
      method: request.method,
      headers,
      // request.signal so a browser disconnect cancels upstream too — needed
      // for the long-lived /stream SSE passthrough.
      signal: request.signal,
      body: hasBody ? await request.arrayBuffer() : undefined,
    });
  } catch {
    return json({ error: `Cannot reach ${entry.label ?? entry.host}` }, 502, deps.cors);
  }

  const responseHeaders: Record<string, string> = { ...deps.cors };
  const upstreamType = upstream.headers.get("content-type");
  if (upstreamType !== null) responseHeaders["content-type"] = upstreamType;
  return new Response(upstream.body, {
    status: upstream.status,
    headers: responseHeaders,
  });
};

/**
 * Handle `/api/servers/*`. `segments` are the path parts after "servers".
 * Returns undefined for shapes that don't match (caller falls through to 404).
 */
export const handleServersRoute = async (
  request: Request,
  segments: ReadonlyArray<string>,
  deps: ServersRouteDeps,
): Promise<Response | undefined> => {
  const { store, cors } = deps;
  const method = request.method.toUpperCase();

  if (segments.length === 0) {
    if (method === "GET") {
      const body: Record<string, unknown> = { servers: store.list().map(publicServer) };
      if (store.error !== null) body.error = store.error;
      return json(body, 200, cors);
    }
    if (method === "POST") {
      let body: unknown;
      try {
        body = await request.json();
      } catch {
        return json({ error: "Invalid JSON body" }, 400, cors);
      }
      const parsed = validateServerInput(body);
      if (!parsed.ok) return json({ error: parsed.error }, 400, cors);
      return json({ server: publicServer(store.create(parsed.input)) }, 201, cors);
    }
    return undefined;
  }

  const id = decodeURIComponent(segments[0] ?? "");
  const entry = store.get(id);
  if (entry === undefined) return json({ error: "Unknown server" }, 404, cors);

  if (segments.length === 1) {
    if (method === "PATCH") {
      let body: unknown;
      try {
        body = await request.json();
      } catch {
        return json({ error: "Invalid JSON body" }, 400, cors);
      }
      const parsed = validateServerInput(body);
      if (!parsed.ok) return json({ error: parsed.error }, 400, cors);
      const updated = store.update(id, parsed.input);
      if (updated === undefined) return json({ error: "Unknown server" }, 404, cors);
      return json({ server: publicServer(updated) }, 200, cors);
    }
    if (method === "DELETE") {
      deps.tunnels.close(id);
      store.remove(id);
      return json({ ok: true }, 200, cors);
    }
    return undefined;
  }

  if (segments.length === 2 && segments[1] === "tunnel") {
    if (entry.ssh === null) return json({ error: "Server has no SSH config" }, 400, cors);
    if (method === "POST") {
      try {
        const { localPort } = await deps.tunnels.ensure(entry);
        return json({ ok: true, localPort }, 200, cors);
      } catch (error) {
        return json({ error: error instanceof Error ? error.message : "Tunnel failed" }, 502, cors);
      }
    }
    if (method === "DELETE") {
      deps.tunnels.close(id);
      return json({ ok: true }, 200, cors);
    }
    return undefined;
  }

  if (segments.length >= 2 && segments[1] === "proxy") {
    const path = `/${segments.slice(2).map(decodeURIComponent).join("/")}`;
    if (path === "/") return json({ error: "Missing upstream path" }, 400, cors);
    return proxy(request, entry, path, deps);
  }

  return undefined;
};

/**
 * Handle `ANY /api/gateway/:peer/*` — gateway mode (docs/protocol.md phase
 * 3). `:peer` is a managed-server registry id: the route resolves it to the
 * entry's url + stored credential and forwards exactly like `/:id/proxy/*`,
 * SSH tunnel included. The browser talks only to this node (its bearer is
 * consumed by the node's own auth check); the peer sees just its stored
 * credential. `segments` are the path parts after "gateway": [peerId,
 * ...upstreamPath]. Returns undefined for shapes that don't match.
 */
export const handleGatewayRoute = async (
  request: Request,
  segments: ReadonlyArray<string>,
  deps: ServersRouteDeps,
): Promise<Response | undefined> => {
  if (segments.length === 0) return undefined;
  const id = decodeURIComponent(segments[0] ?? "");
  const entry = deps.store.get(id);
  if (entry === undefined) {
    return json({ error: "Unknown gateway peer" }, 404, deps.cors);
  }
  const path = `/${segments.slice(1).map(decodeURIComponent).join("/")}`;
  if (path === "/") return json({ error: "Missing upstream path" }, 400, deps.cors);
  return proxy(request, entry, path, deps);
};
