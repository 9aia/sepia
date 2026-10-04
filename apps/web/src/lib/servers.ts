import { AuthError } from "./api";
import { getToken } from "./token";
import type { ApiTarget } from "./targets";

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

export interface ManagedServer {
  readonly id: string;
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly auth: ServerAuthPublic | null;
  readonly ssh: ServerSshPublic | null;
}

/** Create/update payload — secrets are real values, SECRET_MASK, or absent. */
export interface ServerInput {
  readonly label: string;
  readonly host: string;
  readonly port: number;
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
