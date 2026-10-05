import { AuthError } from "./api";
import { getToken } from "./token";
import { localTarget, type ApiTarget } from "./targets";

/**
 * Managed servers (Settings → Servers) — the server-side registry behind
 * `/api/servers`. Unlike peers in `lib/nodes.ts` (browser-local, public URL +
 * token the browser holds), managed entries keep credentials on the node
 * serving this UI, encrypted at rest, and support SSH-tunnelled upstreams.
 * Every call reaches the managed node through `/api/servers/:id/proxy`, so
 * secrets never enter the browser.
 */

/** What GET /api/servers returns — `secret` is always the mask, never real. */
export const SECRET_MASK = "••••••••";

export interface ServerAuthPublic {
  readonly type: "token" | "password";
  readonly user?: string;
  readonly secret: string;
}

export interface ServerSshPublic {
  readonly host: string;
  readonly port: number;
  readonly user: string;
  readonly key?: string;
  readonly keyIsPath?: boolean;
}

/** Upstream protocol — what the managed node's host:port speaks. */
export type ServerScheme = "http" | "https";

export interface ManagedServer {
  readonly id: string;
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly scheme: ServerScheme;
  readonly auth: ServerAuthPublic | null;
  readonly ssh: ServerSshPublic | null;
}

/** Create/update payload — secrets are real values, SECRET_MASK, or absent. */
export interface ServerInput {
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly scheme: ServerScheme;
  readonly auth: {
    readonly type: "token" | "password";
    readonly user?: string;
    readonly secret: string;
  } | null;
  readonly ssh: {
    readonly host: string;
    readonly port: number;
    readonly user: string;
    readonly key?: string;
  } | null;
}

const friendlyHttpError = (status: number): string => {
  if (status === 400) return "The server rejected that entry";
  if (status === 404) return "Unknown server";
  if (status >= 500) return "The server hit an error — try again";
  return `Request failed (${status})`;
};

/** Same contract as api.ts's request(), scoped to the local node. */
const serversFetch = async <T>(path: string, init?: RequestInit): Promise<T> => {
  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      headers: {
        ...(init?.body !== undefined ? { "content-type": "application/json" } : undefined),
        ...(getToken() !== null ? { authorization: `Bearer ${getToken()}` } : undefined),
      },
    });
  } catch {
    throw new Error("Can't reach the Sepia server — is it running?");
  }
  if (res.status === 401) throw new AuthError();
  if (!res.ok) {
    let message = friendlyHttpError(res.status);
    try {
      const body = (await res.json()) as { error?: string };
      if (typeof body.error === "string" && body.error !== "") message = body.error;
    } catch {
      // Non-JSON error body — keep the friendly fallback.
    }
    throw new Error(message);
  }
  return (await res.json()) as T;
};

export const listServers = async (): Promise<ManagedServer[]> => {
  const data = await serversFetch<{ servers: ManagedServer[] }>("/api/servers");
  return data.servers;
};

export const createServer = (input: ServerInput): Promise<ManagedServer> =>
  serversFetch<{ server: ManagedServer }>("/api/servers", {
    method: "POST",
    body: JSON.stringify(input),
  }).then((data) => data.server);

export const updateServer = (id: string, input: ServerInput): Promise<ManagedServer> =>
  serversFetch<{ server: ManagedServer }>(`/api/servers/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  }).then((data) => data.server);

export const deleteServer = (id: string): Promise<void> =>
  serversFetch<{ ok: boolean }>(`/api/servers/${encodeURIComponent(id)}`, {
    method: "DELETE",
  }).then(() => undefined);

/**
 * API target that reaches a managed server through the local node's proxy
 * (and its SSH tunnel, when configured — started lazily upstream). Pass it to
 * any `api.ts` call's `target` argument, e.g. `getNode(serverTarget(srv))`.
 */
export const serverTarget = (server: ManagedServer): ApiTarget => ({
  baseUrl: `/api/servers/${encodeURIComponent(server.id)}/proxy`,
  token: getToken(),
  // Generous: an SSH tunnel cold-start is folded into the first proxied call.
  timeoutMs: 12_000,
});

/**
 * Gateway mode (docs/protocol.md phase 3): an ApiTarget that reaches a
 * managed-server registry entry through `ANY /api/gateway/:id/*` — the same
 * credential-injecting forward as `:id/proxy`, mounted for `via: "gateway"`
 * peers in the node registry (lib/nodes.ts). The browser authenticates with
 * the local node's token; the peer only ever sees its stored credential.
 */
export const gatewayTarget = (serverId: string): ApiTarget => ({
  // Prefixed with the local target's baseUrl so a `localNodeUrl` override
  // repoints gateway hops too — the managed registry lives on whichever
  // node this client treats as local. "" keeps it relative (the default).
  baseUrl: `${localTarget().baseUrl}/api/gateway/${encodeURIComponent(serverId)}`,
  token: getToken(),
  // Generous: an SSH tunnel cold-start is folded into the first proxied call.
  timeoutMs: 12_000,
});

export interface ParsedServerHost {
  readonly scheme: ServerScheme;
  readonly host: string;
  /** Explicit `:port` from the input, or null when it only named a host. */
  readonly port: number | null;
}

/**
 * Parse the Settings form's host field, which takes a bare hostname/IP or a
 * full `http(s)://…` address — pasting a URL is the common case, and any
 * scheme or `:port` it carries wins over the defaults. A path is accepted and
 * dropped (the registry stores an origin, matching normalizeNodeUrl). Returns
 * null on unparseable input or a non-http(s) scheme.
 */
export const parseServerHost = (raw: string): ParsedServerHost | null => {
  const trimmed = raw.trim();
  if (trimmed === "" || /\s/.test(trimmed)) return null;
  const hasScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed);
  if (hasScheme && !/^https?:\/\//i.test(trimmed)) return null;
  try {
    // `sepia://` stands in for "no scheme given" — non-special schemes still
    // split authority/port, but don't imply a protocol of their own.
    const url = new URL(hasScheme ? trimmed : `sepia://${trimmed}`);
    if (url.hostname === "") return null;
    return {
      scheme: !hasScheme || url.protocol === "http:" ? "http" : "https",
      host: url.hostname,
      port: url.port === "" ? null : Number(url.port),
    };
  } catch {
    return null;
  }
};
