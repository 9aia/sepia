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
 * The proxy is what keeps secrets server-side: the client calls
 * `/api/servers/x/proxy/api/node` and this route injects the stored
 * Authorization header. The caller's own bearer token is never forwarded.
 */
export interface ServersRouteDeps {
  readonly store: ServerStore;
  readonly tunnels: TunnelManager;
  readonly cors: Record<string, string>;
  /** Injectable for tests. */
  readonly fetchImpl?: typeof fetch;
  /**
   * Upstream ceiling override for tests; `0` disables. Defaults to
   * UPSTREAM_TIMEOUT_MS — applies to plain requests only, never SSE.
   */
  readonly timeoutMs?: number;
}

const json = (body: unknown, status: number, cors: Record<string, string>): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...cors },
  });

/**
 * Ceiling for a plain proxied call — a peer that holds the socket open
 * without answering must not pin a gateway request forever. Generous on
 * purpose: `POST /api/sessions/:id/prompt` only responds at end-of-turn, so
 * a tight bound would kill live agent runs (the web client aborts gateway
 * hops at 12s anyway; this guards non-UI callers and dead sockets).
 */
const UPSTREAM_TIMEOUT_MS = 5 * 60_000;

/**
 * Cap on a buffered request body — proxied payloads are prompt-sized (the
 * composer's 8MB budget, cf. MAX_PROMPT_PART_CHARS in routes/sessions.ts).
 * Past that, `request.arrayBuffer()` would be allocating hostile RAM.
 */
const MAX_PROXY_BODY_BYTES = 8 * 1024 * 1024;

/**
 * Upstream paths whose response stays open indefinitely — the node's own
 * AG-UI SSE routes (same shapes app.ts's QUERY_TOKEN_PATH authorizes query
 * tokens for). They get no timeout: they only end on client disconnect.
 */
const SSE_UPSTREAM_PATH = /^\/api\/(?:events|sessions\/[^/]+\/stream)$/;

const isStreamRequest = (request: Request, upstreamPath: string): boolean =>
  SSE_UPSTREAM_PATH.test(upstreamPath) ||
  (request.headers.get("accept")?.includes("text/event-stream") ?? false);

/**
 * Buffer the inbound body under a hard cap: a declared `content-length`
 * over the limit rejects cheaply, a lied/absent one trips mid-read —
 * either way a hostile caller can't make the proxy allocate unboundedly.
 * Returns null when the cap is hit.
 */
const readProxyBody = async (request: Request): Promise<Uint8Array | null> => {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_PROXY_BODY_BYTES) return null;
  if (request.body === null) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > MAX_PROXY_BODY_BYTES) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
};

/** Capped JSON body for the management routes — same bound as the proxy. */
const readJsonCapped = async (
  request: Request,
): Promise<
  | { readonly ok: true; readonly value: unknown }
  | { readonly ok: false; readonly status: 400 | 413 }
> => {
  const bytes = await readProxyBody(request);
  if (bytes === null) return { ok: false, status: 413 };
  try {
    return { ok: true, value: JSON.parse(new TextDecoder().decode(bytes)) };
  } catch {
    return { ok: false, status: 400 };
  }
};

/**
 * Direct URL, or the loopback end of the SSH forward when configured.
 *
 * `scheme` describes what `host:port` speaks on the far end, so it applies in
 * both modes: an https entry behind a tunnel gets TLS-over-SSH — `ssh -L`
 * forwards raw TCP, so whatever terminates the remote port (the sepia server
 * itself or its TLS proxy) sees a normal TLS handshake. One caveat: the TLS
 * handshake then names `127.0.0.1`, so certificate verification only passes
 * for certs covering the loopback address — in practice the SSH channel
 * already encrypts, and tunneled entries should almost always use `http`.
 */
const upstreamBase = async (entry: ServerEntry, deps: ServersRouteDeps): Promise<string> => {
  if (entry.ssh === null) return `${entry.scheme}://${entry.host}:${entry.port}`;
  const { localPort } = await deps.tunnels.ensure(entry);
  return `${entry.scheme}://127.0.0.1:${localPort}`;
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
  // The proxy exists to reach the managed node's sepia API — constrain the
  // forwarded path to /api/* on the entry's own origin. Building through
  // `new URL` normalizes `..` segments and `//host` smuggling before the
  // check, so a crafted path can't escape to other routes (or hosts) on the
  // managed port.
  let target: InstanceType<typeof URL>;
  try {
    target = new URL(`${base}${path}${url.search}`);
  } catch {
    return json({ error: "Invalid upstream path" }, 400, deps.cors);
  }
  if (
    target.origin !== new URL(base).origin ||
    !(target.pathname === "/api" || target.pathname.startsWith("/api/"))
  ) {
    return json({ error: "Only /api/* paths can be proxied" }, 400, deps.cors);
  }

  const headers: Record<string, string> = {};
  const contentType = request.headers.get("content-type");
  if (contentType !== null) headers["content-type"] = contentType;
  const auth = authHeader(entry.auth);
  if (auth !== null) headers.authorization = auth;

  const hasBody = request.method !== "GET" && request.method !== "HEAD";
  let body: Uint8Array | undefined;
  if (hasBody) {
    let buffered: Uint8Array | null;
    try {
      buffered = await readProxyBody(request);
    } catch {
      return json({ error: "Failed to read request body" }, 400, deps.cors);
    }
    if (buffered === null) {
      return json(
        { error: `Body exceeds the ${MAX_PROXY_BODY_BYTES}-byte proxy limit` },
        413,
        deps.cors,
      );
    }
    body = buffered;
  }

  // SSE passthroughs stay open by design — they only cancel on client
  // disconnect (request.signal). Everything else rides a combined signal
  // with an upstream ceiling so a dead peer fails the request instead of
  // pinning it forever.
  const streamed = isStreamRequest(request, target.pathname);
  const timeoutMs = deps.timeoutMs ?? UPSTREAM_TIMEOUT_MS;
  const deadline = streamed || timeoutMs <= 0 ? undefined : AbortSignal.timeout(timeoutMs);
  const signal =
    deadline === undefined ? request.signal : AbortSignal.any([request.signal, deadline]);

  let upstream: Response;
  try {
    // `${base}${normalized path}${query}` — keeps the entry's explicit port
    // (URL.toString would elide scheme-default ports like https :443) while
    // the origin/path checks above already pinned the request to /api/*.
    upstream = await fetchImpl(`${base}${target.pathname}${target.search}`, {
      method: request.method,
      headers,
      signal,
      body,
    });
  } catch {
    // The deadline firing without a client abort means the peer never
    // answered — 504, distinct from an unreachable peer's 502.
    const timedOut = deadline?.aborted === true && !request.signal.aborted;
    const label = entry.label ?? entry.host;
    return json(
      { error: timedOut ? `Timed out waiting for ${label}` : `Cannot reach ${label}` },
      timedOut ? 504 : 502,
      deps.cors,
    );
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
      const read = await readJsonCapped(request);
      if (!read.ok) {
        return json(
          { error: read.status === 413 ? "Body too large" : "Invalid JSON body" },
          read.status,
          cors,
        );
      }
      const body = read.value;
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
      const read = await readJsonCapped(request);
      if (!read.ok) {
        return json(
          { error: read.status === 413 ? "Body too large" : "Invalid JSON body" },
          read.status,
          cors,
        );
      }
      const body = read.value;
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
 * SSH tunnel included. The client talks only to this node (its bearer is
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
