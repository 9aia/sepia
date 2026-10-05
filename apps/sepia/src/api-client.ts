import { Effect } from "effect";
import type { PromptPart } from "sepia-acp";
import type { CheckpointRef } from "sepia-core";
import type {
  AgentInfo,
  AttachResult,
  HistoryPage,
  RestoreResult,
  RewindResult,
  SessionSummary,
} from "sepia-session-control";

/**
 * Where a node op lands — the CLI's analogue of the web client's
 * `ApiTarget`/`localTarget` (apps/web/src/lib/targets.ts). The default node
 * is the local machine's server; `SEPIA_NODE_URL`/`--node` repoints every
 * call, `SEPIA_TOKEN`/`--token` authenticates it. The token is read lazily
 * per call so it stays bound to whatever node the command targets.
 */
export interface NodeTarget {
  readonly baseUrl: string;
  readonly token: string | null;
}

export const DEFAULT_NODE_URL = "http://127.0.0.1:8787";

export const defaultNodeUrl = (): string => {
  const env = process.env.SEPIA_NODE_URL;
  return env !== undefined && env.trim() !== "" ? env.trim() : DEFAULT_NODE_URL;
};

export const resolveTarget = (node: string, token: string | undefined): NodeTarget => ({
  baseUrl: node.replace(/\/+$/, ""),
  token: token ?? process.env.SEPIA_TOKEN ?? null,
});

/**
 * A non-OK API response with the server's payload decoded — `status` is the
 * HTTP code, `code` the ControlError tag (`invalid`, `locked`, `busy`…)
 * when the body carried one. Mirrors apps/web/src/lib/api.ts `ApiError`.
 */
export class ApiError extends Error {
  readonly _tag = "ApiError";
  constructor(
    message: string,
    readonly status: number | undefined,
    readonly code?: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const friendlyHttpError = (status: number): string => {
  if (status === 400) return "The server rejected the request";
  if (status === 401) return "Unauthorized — pass --token or set SEPIA_TOKEN";
  if (status === 403) return "Access denied";
  if (status === 404) return "Not found";
  if (status === 409) return "That operation is busy or locked — try again in a moment";
  if (status >= 500) return "The server hit an error — try again";
  return `Request failed (${status})`;
};

/** Prefer the server's `{error, code}` payload over generic status text. */
const toApiError = async (res: Response): Promise<ApiError> => {
  try {
    const body: unknown = await res.json();
    if (isRecord(body) && typeof body.error === "string" && body.error !== "") {
      return typeof body.code === "string" && body.code !== ""
        ? new ApiError(body.error, res.status, body.code)
        : new ApiError(body.error, res.status);
    }
  } catch {
    // Non-JSON or unreadable body — fall back to the friendly status text.
  }
  return new ApiError(friendlyHttpError(res.status), res.status);
};

const asApiError = (cause: unknown, baseUrl: string): ApiError =>
  cause instanceof ApiError
    ? cause
    : new ApiError(
        `Cannot reach ${baseUrl} — is a sepia node running there? (${cause instanceof Error ? cause.message : String(cause)})`,
        undefined,
      );

const headers = (target: NodeTarget, json: boolean): Record<string, string> => ({
  ...(json ? { "content-type": "application/json" } : {}),
  ...(target.token !== null ? { authorization: `Bearer ${target.token}` } : {}),
});

/** Session ids collide across agents; `?agent=` scopes the server's lookup. */
const agentQuery = (agent?: string): string =>
  agent === undefined || agent === "" ? "" : `?agent=${encodeURIComponent(agent)}`;

/**
 * One JSON request against `target`, unwrapping the server's error payload
 * into `ApiError`. `body === undefined` sends no body (GET/DELETE).
 */
export const request = <A>(
  target: NodeTarget,
  method: string,
  path: string,
  body?: unknown,
): Effect.Effect<A, ApiError> =>
  Effect.tryPromise({
    try: async () => {
      const res = await fetch(`${target.baseUrl}${path}`, {
        method,
        headers: headers(target, body !== undefined),
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw await toApiError(res);
      const text = await res.text();
      return (text === "" ? undefined : (JSON.parse(text) as unknown)) as A;
    },
    catch: (cause) => asApiError(cause, target.baseUrl),
  });

// --- Node -------------------------------------------------------------------

export interface NodeDescriptor {
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly protocol: number;
  readonly agents: ReadonlyArray<string>;
  readonly capabilities: ReadonlyArray<string>;
}

export interface UserInfo {
  readonly username: string;
  readonly homedir: string;
  readonly shell: string | null;
  readonly hostname: string;
  readonly platform: string;
  readonly arch: string;
}

export interface Project {
  readonly id: string;
  readonly name: string;
}

export interface ServerEntry {
  readonly id: string;
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly scheme: "http" | "https";
  readonly auth: unknown;
  readonly ssh: unknown;
}

export const getHealth = (target: NodeTarget) =>
  request<{ readonly ok: boolean; readonly db: boolean }>(target, "GET", "/api/health");

export const getNode = (target: NodeTarget) => request<NodeDescriptor>(target, "GET", "/api/node");

export const getUser = (target: NodeTarget) =>
  request<{ readonly user: UserInfo }>(target, "GET", "/api/user");

export const listDirs = (target: NodeTarget, path: string) =>
  Effect.map(
    request<{ readonly dirs: ReadonlyArray<string> }>(
      target,
      "GET",
      `/api/fs?path=${encodeURIComponent(path)}`,
    ),
    (data) => data.dirs,
  );

export const listAgents = (target: NodeTarget) =>
  Effect.map(
    request<{ readonly agents: ReadonlyArray<AgentInfo> }>(target, "GET", "/api/agents"),
    (data) => data.agents,
  );

export const pairRedeem = (target: NodeTarget, code: string) =>
  request<{ readonly token: string }>(target, "POST", "/api/pair", { code });

// --- Sessions ---------------------------------------------------------------

export const listSessions = (target: NodeTarget, options?: { readonly withLocks?: boolean }) =>
  Effect.map(
    request<{ readonly sessions: ReadonlyArray<SessionSummary> }>(
      target,
      "GET",
      `/api/sessions${options?.withLocks === true ? "?withLocks=1" : ""}`,
    ),
    (data) => data.sessions,
  );

export const createSession = (
  target: NodeTarget,
  input: {
    readonly cwd: string;
    readonly agent?: string;
    readonly title?: string;
    readonly model?: string;
    readonly fallbacks?: ReadonlyArray<string>;
  },
) =>
  request<{ readonly id: string; readonly agentId: string }>(
    target,
    "POST",
    "/api/sessions",
    input,
  );

export const getHistory = (
  target: NodeTarget,
  id: string,
  options?: { readonly limit?: number; readonly before?: number; readonly agent?: string },
) => {
  const params = new URLSearchParams();
  if (options?.limit !== undefined) params.set("limit", String(options.limit));
  if (options?.before !== undefined) params.set("before", String(options.before));
  if (options?.agent !== undefined) params.set("agent", options.agent);
  const query = params.size === 0 ? "" : `?${params.toString()}`;
  return request<HistoryPage>(
    target,
    "GET",
    `/api/sessions/${encodeURIComponent(id)}/history${query}`,
  );
};

export const getCheckpoints = (target: NodeTarget, id: string, agent?: string) =>
  Effect.map(
    request<{ readonly checkpoints: ReadonlyArray<CheckpointRef> }>(
      target,
      "GET",
      `/api/sessions/${encodeURIComponent(id)}/checkpoints${agentQuery(agent)}`,
    ),
    (data) => data.checkpoints,
  );

/** The complete session IR — `{session}` on the wire, unwrapped here. */
export const exportSession = (target: NodeTarget, id: string, agent?: string) =>
  Effect.map(
    request<{ readonly session: Record<string, unknown> }>(
      target,
      "GET",
      `/api/sessions/${encodeURIComponent(id)}/export${agentQuery(agent)}`,
    ),
    (data) => data.session,
  );

export const attach = (
  target: NodeTarget,
  id: string,
  options?: {
    readonly takeover?: boolean;
    readonly model?: string;
    readonly fallbacks?: ReadonlyArray<string>;
    readonly agent?: string;
  },
) =>
  request<AttachResult>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/attach${agentQuery(options?.agent)}`,
    {
      ...(options?.takeover === true ? { takeover: true } : {}),
      ...(options?.model !== undefined ? { model: options.model } : {}),
      ...(options?.fallbacks !== undefined && options.fallbacks.length > 0
        ? { fallbacks: options.fallbacks }
        : {}),
    },
  );

export const prompt = (
  target: NodeTarget,
  id: string,
  text: string,
  attachments?: ReadonlyArray<PromptPart>,
  agent?: string,
) =>
  request<{ readonly ok: boolean }>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/prompt${agentQuery(agent)}`,
    attachments === undefined || attachments.length === 0 ? { text } : { text, attachments },
  );

export const cancel = (target: NodeTarget, id: string, agent?: string) =>
  request<{ readonly ok: boolean }>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/cancel${agentQuery(agent)}`,
    {},
  );

export const respondToPermission = (
  target: NodeTarget,
  id: string,
  requestId: string,
  optionId: string | null,
  agent?: string,
) =>
  request<{ readonly ok: boolean }>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/permission${agentQuery(agent)}`,
    { requestId, optionId },
  );

export const deleteSession = (target: NodeTarget, id: string, agent?: string) =>
  request<{ readonly ok: boolean }>(
    target,
    "DELETE",
    `/api/sessions/${encodeURIComponent(id)}${agentQuery(agent)}`,
  );

export interface SessionMetaPatch {
  readonly title?: string;
  readonly pinned?: boolean;
  readonly archived?: boolean;
  readonly projectIds?: ReadonlyArray<string>;
  readonly model?: string | null;
}

export const patchSession = (
  target: NodeTarget,
  id: string,
  patch: SessionMetaPatch,
  agent?: string,
) =>
  request<{ readonly ok: boolean }>(
    target,
    "PATCH",
    `/api/sessions/${encodeURIComponent(id)}${agentQuery(agent)}`,
    patch,
  );

export const convertSession = (target: NodeTarget, id: string, to: "cline" | "devin") =>
  request<{ readonly sessionId: string }>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/convert`,
    { agent: to },
  );

export const importSession = (
  target: NodeTarget,
  input: {
    readonly agent: "cline" | "devin";
    readonly session?: unknown;
    readonly history?: ReadonlyArray<unknown>;
    readonly cwd?: string;
    readonly title?: string;
    readonly model?: string;
  },
) => request<SessionSummary>(target, "POST", "/api/sessions/import", input);

export const restoreSession = (
  target: NodeTarget,
  id: string,
  selector:
    | { readonly path: string; readonly toolCallId?: string }
    | { readonly checkpoint: string; readonly paths?: ReadonlyArray<string> },
  agent?: string,
) =>
  request<RestoreResult>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/restore${agentQuery(agent)}`,
    { confirm: true, ...selector },
  );

export const rewindSession = (
  target: NodeTarget,
  id: string,
  selector:
    | { readonly nodeId: number }
    | { readonly turns: number }
    | { readonly checkpoint: string },
  agent?: string,
) =>
  request<RewindResult>(
    target,
    "POST",
    `/api/sessions/${encodeURIComponent(id)}/rewind${agentQuery(agent)}`,
    { confirm: true, ...selector },
  );

// --- Meta / config / projects ------------------------------------------------

export const getConfig = (target: NodeTarget) =>
  Effect.map(
    request<{ readonly config: Record<string, unknown> }>(target, "GET", "/api/config"),
    (data) => data.config,
  );

export const setConfig = (target: NodeTarget, key: string, value: unknown) =>
  request<{ readonly key: string; readonly value: unknown }>(
    target,
    "PATCH",
    `/api/config/${encodeURIComponent(key)}`,
    { value },
  );

export const listProjects = (target: NodeTarget) =>
  Effect.map(
    request<{ readonly projects: ReadonlyArray<Project> }>(target, "GET", "/api/projects"),
    (data) => data.projects,
  );

export const createProject = (target: NodeTarget, name: string) =>
  Effect.map(
    request<{ readonly project: Project }>(target, "POST", "/api/projects", { name }),
    (data) => data.project,
  );

export const renameProject = (target: NodeTarget, id: string, name: string) =>
  request<{ readonly ok: boolean }>(target, "PATCH", `/api/projects/${encodeURIComponent(id)}`, {
    name,
  });

export const deleteProject = (target: NodeTarget, id: string) =>
  request<{ readonly ok: boolean }>(target, "DELETE", `/api/projects/${encodeURIComponent(id)}`);

// --- Project transfer (docs/protocol.md "Project transfer") -------------------

/** A peer node's `{url, token}` — the credential a node authenticates with there. */
export interface TransferEndpoint {
  readonly url: string;
  readonly token?: string;
}

/** What a bundle import reports back (POST /api/projects/import or a pull/push `done` frame). */
export interface ProjectImportSummary {
  readonly project: Project;
  readonly imported: ReadonlyArray<{
    readonly id: string;
    readonly sourceId: string;
    readonly agent: string;
    readonly title: string;
  }>;
  readonly skipped: ReadonlyArray<{ readonly id: string; readonly error: string }>;
  readonly truncated: boolean;
}

/** GET /api/projects/:id/export — the NDJSON bundle, returned raw. */
export const exportProject = (target: NodeTarget, id: string) =>
  Effect.tryPromise({
    try: async () => {
      const res = await fetch(`${target.baseUrl}/api/projects/${encodeURIComponent(id)}/export`, {
        headers: headers(target, false),
      });
      if (!res.ok) throw await toApiError(res);
      return res.text();
    },
    catch: (cause) => asApiError(cause, target.baseUrl),
  });

/** POST /api/projects/import — upload an NDJSON bundle; the server streams the parse. */
export const importProject = (target: NodeTarget, bundle: string) =>
  Effect.tryPromise({
    try: async () => {
      const res = await fetch(`${target.baseUrl}/api/projects/import`, {
        method: "POST",
        headers: {
          "content-type": "application/x-ndjson",
          ...(target.token !== null ? { authorization: `Bearer ${target.token}` } : {}),
        },
        body: bundle,
      });
      if (!res.ok) throw await toApiError(res);
      return (await res.json()) as ProjectImportSummary;
    },
    catch: (cause) => asApiError(cause, target.baseUrl),
  });

/**
 * POST /api/projects/pull — the receiving node fetches `{source.url}`'s
 * export with `source.token` and imports it; progress arrives as SSE frames
 * (`start`, `session`, `done`, `error`).
 */
export const pullProject = (
  target: NodeTarget,
  input: { readonly source: TransferEndpoint; readonly project: string },
  onFrame: (frame: SseFrame) => void,
) => streamSse(target, "/api/projects/pull", onFrame, { method: "POST", body: input });

/**
 * POST /api/projects/:id/push — this node bundles the project and POSTs it
 * to `{target.url}`'s /api/projects/import with `target.token`.
 */
export const pushProject = (
  target: NodeTarget,
  id: string,
  input: { readonly target: TransferEndpoint },
  onFrame: (frame: SseFrame) => void,
) =>
  streamSse(target, `/api/projects/${encodeURIComponent(id)}/push`, onFrame, {
    method: "POST",
    body: input,
  });

// --- Managed servers ----------------------------------------------------------

export const listServers = (target: NodeTarget) =>
  request<{ readonly servers: ReadonlyArray<ServerEntry>; readonly error?: string }>(
    target,
    "GET",
    "/api/servers",
  );

export interface ServerInput {
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly scheme: "http" | "https";
  readonly auth:
    | { readonly type: "token"; readonly secret: string }
    | { readonly type: "password"; readonly user?: string; readonly secret: string }
    | null;
  readonly ssh: {
    readonly host: string;
    readonly port: number;
    readonly user: string;
    readonly key?: string;
  } | null;
}

export const createServer = (target: NodeTarget, input: ServerInput) =>
  Effect.map(
    request<{ readonly server: ServerEntry }>(target, "POST", "/api/servers", input),
    (data) => data.server,
  );

export const updateServer = (target: NodeTarget, id: string, input: ServerInput) =>
  Effect.map(
    request<{ readonly server: ServerEntry }>(
      target,
      "PATCH",
      `/api/servers/${encodeURIComponent(id)}`,
      input,
    ),
    (data) => data.server,
  );

export const removeServer = (target: NodeTarget, id: string) =>
  request<{ readonly ok: boolean }>(target, "DELETE", `/api/servers/${encodeURIComponent(id)}`);

export const tunnelUp = (target: NodeTarget, id: string) =>
  request<{ readonly ok: boolean; readonly localPort: number }>(
    target,
    "POST",
    `/api/servers/${encodeURIComponent(id)}/tunnel`,
    {},
  );

export const tunnelDown = (target: NodeTarget, id: string) =>
  request<{ readonly ok: boolean }>(
    target,
    "DELETE",
    `/api/servers/${encodeURIComponent(id)}/tunnel`,
  );

// --- Push ---------------------------------------------------------------------

export const getVapid = (target: NodeTarget) =>
  request<{ readonly publicKey: string }>(target, "GET", "/api/push/vapid");

export const pushSubscribe = (target: NodeTarget, subscription: unknown) =>
  request<{ readonly ok: boolean }>(target, "POST", "/api/push/subscribe", subscription);

export const pushUnsubscribe = (target: NodeTarget, endpoint: string) =>
  request<{ readonly ok: boolean }>(target, "DELETE", "/api/push/subscribe", { endpoint });

// --- SSE ----------------------------------------------------------------------

/** One parsed SSE frame — the `event:` name (absent on unnamed frames) plus the joined `data:` lines. */
export interface SseFrame {
  readonly event: string | undefined;
  readonly data: string;
}

const parseFrame = (raw: string): SseFrame | null => {
  let event: string | undefined;
  const data: Array<string> = [];
  for (const line of raw.split("\n")) {
    if (line === "" || line.startsWith(":")) continue;
    if (line.startsWith("event:")) event = line.slice("event:".length).trim();
    else if (line.startsWith("data:")) data.push(line.slice("data:".length).trim());
  }
  return data.length === 0 && event === undefined ? null : { event, data: data.join("\n") };
};

/**
 * GET an SSE endpoint and invoke `onFrame` per frame until the server closes
 * the stream. Interrupting the effect aborts the fetch. `?access_token` is
 * the server's documented auth path for these two routes, but the bearer
 * header works for fetch clients and is what we send.
 */
export const streamSse = (
  target: NodeTarget,
  path: string,
  onFrame: (frame: SseFrame) => void,
  options?: { readonly method?: string; readonly body?: unknown },
): Effect.Effect<void, ApiError> =>
  Effect.async<void, ApiError>((resume) => {
    const controller = new AbortController();
    void (async () => {
      try {
        const res = await fetch(`${target.baseUrl}${path}`, {
          method: options?.method ?? "GET",
          headers: {
            accept: "text/event-stream",
            ...headers(target, options?.body !== undefined),
          },
          body: options?.body === undefined ? undefined : JSON.stringify(options.body),
          signal: controller.signal,
        });
        if (!res.ok) throw await toApiError(res);
        if (res.body === null) throw new ApiError("The stream had no body", res.status);
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          let at: number;
          while ((at = buffer.indexOf("\n\n")) !== -1) {
            const raw = buffer.slice(0, at);
            buffer = buffer.slice(at + 2);
            const frame = parseFrame(raw);
            if (frame !== null) onFrame(frame);
          }
        }
        resume(Effect.void);
      } catch (cause) {
        resume(
          controller.signal.aborted ? Effect.void : Effect.fail(asApiError(cause, target.baseUrl)),
        );
      }
    })();
    return Effect.sync(() => controller.abort());
  });
