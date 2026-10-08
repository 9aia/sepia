import type {
  AgentCapabilities,
  AgentInfo,
  AttachResult,
  CreateSessionInput,
  HistoryMessage,
  HistoryPage,
  NodeDescriptor,
  RestoreResult,
  RewindResult,
  SessionCheckpoint,
  SessionSummary,
  UserInfo,
} from "./types";
import type { PromptPart } from "./attachments";
import { localTarget, type ApiTarget } from "./targets";
import { setAuthBlocked } from "./store";
import { setCookieAuth, setToken as setStoredToken } from "./token";

export { getToken, setToken, isCookieAuth, setCookieAuth } from "./token";
export type { ApiTarget } from "./targets";

export class AuthError extends Error {
  constructor() {
    super("Unauthorized");
    this.name = "AuthError";
  }
}

/**
 * A non-OK API response with the server's payload decoded — `status` is the
 * HTTP code, `code` the ControlError tag (`invalid`, `locked`, `busy`…)
 * when the body carried one, so callers can branch on the failure kind.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  constructor(message: string, status: number, code?: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

const friendlyHttpError = (status: number): string => {
  if (status === 400) return "The node rejected the request";
  if (status === 403) return "Access denied";
  if (status === 404) return "Not found";
  if (status === 409) return "That operation is busy — try again in a moment";
  if (status >= 500) return "The node hit an error — try again";
  return `Request failed (${status})`;
};

/**
 * Every API call targets a node: `target.baseUrl` prefixes the path ("" for
 * the same-origin server) and `target.token` authenticates it. The default
 * target is the local node, so same-origin call sites never pass one.
 */
async function sepiaFetch(
  path: string,
  init?: RequestInit,
  target: ApiTarget = localTarget(),
): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(`${target.baseUrl}${path}`, {
      ...init,
      signal: target.timeoutMs === undefined ? init?.signal : AbortSignal.timeout(target.timeoutMs),
      headers: {
        ...(init?.body !== undefined ? { "content-type": "application/json" } : undefined),
        ...(target.token !== null ? { authorization: `Bearer ${target.token}` } : undefined),
      },
    });
  } catch {
    throw new Error("Can't reach the Sepia node — is it running?");
  }
  if (res.status === 401) {
    // Peer 401s are that node's own auth problem — only the local target's
    // rejection gates the UI (and instantly, not after a query settles).
    if (target.baseUrl === localTarget().baseUrl) setAuthBlocked(true);
    throw new AuthError();
  }
  return res;
}

async function request<T>(path: string, init?: RequestInit, target?: ApiTarget): Promise<T> {
  const res = await sepiaFetch(path, init, target);
  if (!res.ok) {
    const { message, code } = await responseError(res);
    throw new ApiError(message, res.status, code);
  }
  return (await res.json()) as T;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * `POST /api/auth/login` — exchange a presented token for the httpOnly
 * `sepia_token` cookie. The cookie rides every subsequent same-origin
 * call automatically, so a successful login leaves NO token in JS:
 * cookieAuth flips on (localTarget stops sending Bearer) and the legacy
 * localStorage token is dropped. Throws AuthError on a wrong token.
 */
export async function login(token: string): Promise<void> {
  const res = await fetch("/api/auth/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (res.status === 401) throw new AuthError();
  if (!res.ok) throw new ApiError("Login failed", res.status);
  setCookieAuth(true);
  // The cookie holds the credential now — scrub the JS-side copy.
  setStoredToken(null);
}

/** `POST /api/auth/logout` — expire the cookie server-side. */
export async function logout(): Promise<void> {
  await fetch("/api/auth/logout", { method: "POST" }).catch(() => {});
  setCookieAuth(false);
}

/**
 * The server's `{error, code}` payload carries the real failure ("Session
 * is not attached", "held by another process") — prefer it over the generic
 * status text, which stays as the fallback for non-JSON bodies.
 */
const responseError = async (res: Response): Promise<{ message: string; code?: string }> => {
  try {
    const body: unknown = await res.json();
    if (isRecord(body) && typeof body.error === "string" && body.error !== "") {
      return typeof body.code === "string" && body.code !== ""
        ? { message: body.error, code: body.code }
        : { message: body.error };
    }
  } catch {
    // Non-JSON or unreadable body — fall back to the friendly status text.
  }
  return { message: friendlyHttpError(res.status) };
};

// Session ids collide across agents; `?agent=` scopes the server's lookup.
const agentQuery = (agent?: string): string =>
  agent === undefined || agent === "" ? "" : `?agent=${encodeURIComponent(agent)}`;

export async function getNode(target?: ApiTarget): Promise<NodeDescriptor> {
  return request<NodeDescriptor>("/api/node", undefined, target);
}

/**
 * POST /api/pair — redeem a one-time code printed by `sepia pair` for a
 * long-lived credential (docs/protocol.md). Unauthenticated by design: the
 * code authorizes the exchange, the returned token authenticates later calls.
 */
export async function pairNode(
  code: string,
  target: ApiTarget,
  options?: { readonly forwardTargetAuth?: boolean },
): Promise<{ token: string }> {
  const res = await sepiaFetch(
    "/api/pair",
    { method: "POST", body: JSON.stringify({ code }) },
    // Pairing is pre-credential — never send a stored token along, unless the
    // call itself rides an authenticated hop (this node's /api/gateway route,
    // where the local bearer authorizes the forward, not the peer).
    options?.forwardTargetAuth === true ? target : { ...target, token: null },
  );
  if (res.status === 404) {
    throw new Error("That code didn't work — it may have expired or already been used");
  }
  if (!res.ok) {
    throw new Error(friendlyHttpError(res.status));
  }
  return (await res.json()) as { token: string };
}

export async function listSessions(
  target?: ApiTarget,
  options?: { readonly withLocks?: boolean },
): Promise<SessionSummary[]> {
  const query = options?.withLocks === true ? "?withLocks=1" : "";
  const data = await request<{ sessions: SessionSummary[] }>(
    `/api/sessions${query}`,
    undefined,
    target,
  );
  return data.sessions;
}

export async function createSession(
  input: CreateSessionInput,
  target?: ApiTarget,
): Promise<{ id: string; agentId?: string; capabilities?: AgentCapabilities }> {
  return request<{ id: string; agentId?: string; capabilities?: AgentCapabilities }>(
    "/api/sessions",
    {
      method: "POST",
      body: JSON.stringify(input),
    },
    target,
  );
}

export async function getHistory(
  id: string,
  options?: { limit?: number; before?: number; agent?: string },
  target?: ApiTarget,
): Promise<HistoryPage> {
  const params = new URLSearchParams();
  if (options?.limit !== undefined) params.set("limit", String(options.limit));
  if (options?.before !== undefined) params.set("before", String(options.before));
  if (options?.agent !== undefined) params.set("agent", options.agent);
  const query = params.size === 0 ? "" : `?${params.toString()}`;
  return request<HistoryPage>(
    `/api/sessions/${encodeURIComponent(id)}/history${query}`,
    undefined,
    target,
  );
}

export interface AttachOptions {
  readonly takeover?: boolean;
  readonly model?: string;
  readonly fallbacks?: ReadonlyArray<string>;
  readonly agent?: string;
}

export async function attach(
  id: string,
  options?: AttachOptions,
  target?: ApiTarget,
): Promise<AttachResult> {
  const body: Record<string, unknown> = {};
  if (options?.takeover === true) body.takeover = true;
  if (options?.model !== undefined) body.model = options.model;
  if (options?.fallbacks !== undefined) body.fallbacks = options.fallbacks;
  return request<AttachResult>(
    `/api/sessions/${encodeURIComponent(id)}/attach${agentQuery(options?.agent)}`,
    {
      method: "POST",
      body: Object.keys(body).length === 0 ? undefined : JSON.stringify(body),
    },
    target,
  );
}

/**
 * GET /api/sessions/:id/checkpoints — the workspace snapshot refs the store
 * recorded (Cline shadow-git refs). Cheap companion of /export — refs only.
 */
export async function getCheckpoints(
  id: string,
  agent?: string,
  target?: ApiTarget,
): Promise<SessionCheckpoint[]> {
  const data = await request<{ checkpoints: SessionCheckpoint[] }>(
    `/api/sessions/${encodeURIComponent(id)}/checkpoints${agentQuery(agent)}`,
    undefined,
    target,
  );
  return data.checkpoints;
}

/**
 * A restore call's selector: `{path, toolCallId?}` reverts the file through
 * the session's recorded diffs; `{checkpoint, paths?}` materializes the
 * files a recorded shadow-git ref covers. `confirm` is mandatory.
 */
export type RestoreSelector =
  | { readonly path: string; readonly toolCallId?: string }
  | { readonly checkpoint: string; readonly paths?: ReadonlyArray<string> };

/**
 * POST /api/sessions/:id/restore — writes/deletes real files under the
 * session's working directory on the owning node. The server refuses while
 * the session is busy or locked by a live process.
 */
export async function restoreSession(
  id: string,
  selector: RestoreSelector,
  agent?: string,
  target?: ApiTarget,
): Promise<RestoreResult> {
  return request<RestoreResult>(
    `/api/sessions/${encodeURIComponent(id)}/restore${agentQuery(agent)}`,
    {
      method: "POST",
      body: JSON.stringify({ confirm: true, ...selector }),
    },
    target,
  );
}

/**
 * A rewind call's selector: `{nodeId}` truncates the transcript after that
 * node (a history row's `nodeId`); `{turns: n}` drops the last n user
 * turns; `{checkpoint}` rewinds to a recorded snapshot ref. `confirm` is
 * mandatory; the server refuses while the session is busy or held.
 */
export type RewindSelector =
  | { readonly nodeId: number }
  | { readonly turns: number }
  | { readonly checkpoint: string };

/**
 * POST /api/sessions/:id/rewind — deletes stored conversation, not files
 * (that's `restoreSession`). The session ends at the selector's point; a
 * live attach is detached first.
 */
export async function rewindSession(
  id: string,
  selector: RewindSelector,
  agent?: string,
  target?: ApiTarget,
): Promise<RewindResult> {
  return request<RewindResult>(
    `/api/sessions/${encodeURIComponent(id)}/rewind${agentQuery(agent)}`,
    {
      method: "POST",
      body: JSON.stringify({ confirm: true, ...selector }),
    },
    target,
  );
}

export async function listDirs(path: string): Promise<string[]> {
  const data = await request<{ dirs: string[] }>(`/api/fs?path=${encodeURIComponent(path)}`);
  return data.dirs;
}

export async function getUserInfo(): Promise<UserInfo> {
  const data = await request<{ user: UserInfo }>("/api/user");
  return data.user;
}

export async function listAgents(target?: ApiTarget): Promise<AgentInfo[]> {
  const data = await request<{ agents: AgentInfo[] }>("/api/agents", undefined, target);
  return data.agents;
}

export interface SessionMetaPatch {
  title?: string;
  pinned?: boolean;
  archived?: boolean;
  projectIds?: string[];
  model?: string | null;
}

export async function getConfig(): Promise<{ config: Record<string, unknown> }> {
  return request("/api/config");
}

export async function setConfigKey(key: string, value: unknown): Promise<void> {
  await request(`/api/config/${encodeURIComponent(key)}`, {
    method: "PATCH",
    body: JSON.stringify({ value }),
  });
}

export async function patchSessionMeta(
  id: string,
  patch: SessionMetaPatch,
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const res = await sepiaFetch(
    `/api/sessions/${encodeURIComponent(id)}${agentQuery(agent)}`,
    {
      method: "PATCH",
      body: JSON.stringify(patch),
    },
    target,
  );
  return res.ok;
}

export async function listProjects(
  target?: ApiTarget,
): Promise<{ projects: import("./types").Project[] }> {
  return request(`/api/projects`, undefined, target);
}

export async function createProject(
  name: string,
  target?: ApiTarget,
): Promise<{ project: import("./types").Project }> {
  return request(`/api/projects`, { method: "POST", body: JSON.stringify({ name }) }, target);
}

export async function renameProject(
  id: string,
  name: string,
  target?: ApiTarget,
): Promise<boolean> {
  const res = await sepiaFetch(
    `/api/projects/${encodeURIComponent(id)}`,
    {
      method: "PATCH",
      body: JSON.stringify({ name }),
    },
    target,
  );
  return res.ok;
}

export async function deleteProject(id: string, target?: ApiTarget): Promise<boolean> {
  const res = await sepiaFetch(
    `/api/projects/${encodeURIComponent(id)}`,
    { method: "DELETE" },
    target,
  );
  return res.ok;
}

// --- Project transfer (docs/protocol.md "Project transfer") -------------------

/** A peer node's `{url, token}` — resolved by lib/transfer.ts's peerEndpoint. */
export interface TransferEndpoint {
  readonly url: string;
  readonly token?: string | null;
}

/** The `done` frame payload / POST /api/projects/import response. */
export interface ProjectImportSummary {
  readonly project: { id: string; name: string };
  readonly imported: ReadonlyArray<{
    id: string;
    sourceId: string;
    agent: string;
    title: string;
  }>;
  readonly skipped: ReadonlyArray<{ id: string; error: string }>;
  readonly truncated: boolean;
}

/** One SSE frame off a pull/push stream (`start`, `session`, `done`, `error`). */
export interface TransferFrame {
  readonly event: string;
  readonly data: Record<string, unknown>;
}

/**
 * Read an SSE `Response` body (EventSource can't POST) — resolves on `done`,
 * rejects on an `error` frame or a dead stream.
 */
const readTransferStream = async (
  res: Response,
  onFrame?: (frame: TransferFrame) => void,
): Promise<ProjectImportSummary> => {
  if (res.body === null) throw new Error("The transfer stream had no body");
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let summary: ProjectImportSummary | undefined;
  let failure: string | undefined;
  const handle = (raw: string): void => {
    const event = /^event: (.*)$/m.exec(raw)?.[1];
    const dataLine = /^data: (.*)$/m.exec(raw)?.[1];
    if (event === undefined || dataLine === undefined) return;
    let data: Record<string, unknown> = {};
    try {
      data = JSON.parse(dataLine) as Record<string, unknown>;
    } catch {
      // Malformed frame — skip it like the feed does.
      return;
    }
    onFrame?.({ event, data });
    if (event === "done") summary = data as unknown as ProjectImportSummary;
    if (event === "error") {
      failure = typeof data.error === "string" ? data.error : "The transfer failed";
    }
  };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let at: number;
    while ((at = buffer.indexOf("\n\n")) !== -1) {
      const raw = buffer.slice(0, at);
      buffer = buffer.slice(at + 2);
      handle(raw);
    }
  }
  if (failure !== undefined) throw new Error(failure);
  if (summary === undefined) throw new Error("The transfer stream ended without a result");
  return summary;
};

/**
 * POST /api/projects/pull on `target` (the node receiving the project —
 * default this one). It fetches `source.url`'s export with `source.token`
 * itself; progress frames arrive over the response's SSE stream.
 */
export async function pullProject(
  input: { readonly source: TransferEndpoint; readonly project: string },
  onFrame?: (frame: TransferFrame) => void,
  target?: ApiTarget,
): Promise<ProjectImportSummary> {
  const res = await sepiaFetch(
    "/api/projects/pull",
    {
      method: "POST",
      body: JSON.stringify({
        source: {
          url: input.source.url,
          ...(input.source.token ? { token: input.source.token } : {}),
        },
        project: input.project,
      }),
    },
    target,
  );
  if (!res.ok) {
    const { message, code } = await responseError(res);
    throw new ApiError(message, res.status, code);
  }
  return readTransferStream(res, onFrame);
}

/**
 * POST /api/projects/:id/push on the node that owns the project — it bundles
 * the sessions and POSTs them to `endpoint.url`'s /api/projects/import with
 * `endpoint.token`.
 */
export async function pushProject(
  id: string,
  endpoint: TransferEndpoint,
  onFrame?: (frame: TransferFrame) => void,
  target?: ApiTarget,
): Promise<ProjectImportSummary> {
  const res = await sepiaFetch(
    `/api/projects/${encodeURIComponent(id)}/push`,
    {
      method: "POST",
      body: JSON.stringify({
        target: { url: endpoint.url, ...(endpoint.token ? { token: endpoint.token } : {}) },
      }),
    },
    target,
  );
  if (!res.ok) {
    const { message, code } = await responseError(res);
    throw new ApiError(message, res.status, code);
  }
  return readTransferStream(res, onFrame);
}

export async function convertSession(
  id: string,
  agent: string,
  fromAgent?: string,
  target?: ApiTarget,
): Promise<{ sessionId: string }> {
  return request(
    `/api/sessions/${encodeURIComponent(id)}/convert${agentQuery(fromAgent)}`,
    {
      method: "POST",
      body: JSON.stringify({ agent }),
    },
    target,
  );
}

/**
 * GET /api/sessions/:id/export — the complete session IR (`{session}` —
 * nodes with toolCalls, thinking, usage and parent links). `null` when the
 * node predates the endpoint (404) so callers can fall back to paged
 * history. The payload stays opaque here: it round-trips untouched into
 * `POST /api/sessions/import` on the target node.
 */
export async function getSessionExport(
  id: string,
  options?: { agent?: string },
  target?: ApiTarget,
): Promise<Record<string, unknown> | null> {
  const res = await sepiaFetch(
    `/api/sessions/${encodeURIComponent(id)}/export${agentQuery(options?.agent)}`,
    undefined,
    target,
  );
  if (res.status === 404) return null;
  if (!res.ok) {
    throw new Error(friendlyHttpError(res.status));
  }
  const body = (await res.json()) as { session?: unknown };
  return isRecord(body.session) ? body.session : null;
}

export interface ImportSessionInput {
  readonly agent: string;
  readonly cwd?: string;
  readonly title?: string;
  /** Full IR from GET /api/sessions/:id/export — the preferred form. */
  readonly session?: unknown;
  /** Flat history form — the compat path for older source nodes. */
  readonly history?: ReadonlyArray<HistoryMessage>;
}

/**
 * POST /api/sessions/import — write explicit IR into an agent's store.
 * `session` (the /export payload) is full fidelity; `history` is the flat
 * compat form — the server prefers `session` when both arrive.
 */
export async function importSession(
  input: ImportSessionInput,
  target?: ApiTarget,
): Promise<SessionSummary> {
  return request<SessionSummary>(
    "/api/sessions/import",
    {
      method: "POST",
      body: JSON.stringify(input),
    },
    target,
  );
}

const RESUME_PAGE_SIZE = 500;

/**
 * Every backlog page, oldest → newest. The server pages backwards through
 * `before`; `start > 0` on a page means earlier messages still exist.
 */
export async function fetchAllHistory(
  id: string,
  options?: { agent?: string },
  source?: ApiTarget,
): Promise<HistoryMessage[]> {
  const pages: HistoryMessage[][] = [];
  let before: number | undefined;
  for (;;) {
    const page = await getHistory(
      id,
      { limit: RESUME_PAGE_SIZE, before, agent: options?.agent },
      source,
    );
    pages.unshift(page.messages);
    if (page.start <= 0 || page.messages.length === 0) break;
    before = page.start;
  }
  return pages.flat();
}

export interface ResumeOptions {
  /** Scopes the history lookup — bare ids collide across the source's agents. */
  readonly fromAgent?: string;
  readonly cwd?: string;
  readonly title?: string;
}

/**
 * "Resume on…": pull the session off the source node and replay it into
 * `agent`'s store on the target node via POST /api/sessions/import.
 * Prefers the full IR from GET .../export (tool-call ids/args, thinking,
 * usage, tree links survive); a source node too old to serve it 404s and
 * falls back to paging the flat /history projection — the compat path every
 * node understands. Same node + different agent is the same-machine agent
 * switch; a peer target moves the session to another machine.
 */
export async function resumeSession(
  source: ApiTarget | undefined,
  id: string,
  agent: string,
  target: ApiTarget | undefined,
  options?: ResumeOptions,
): Promise<SessionSummary> {
  const session = await getSessionExport(id, { agent: options?.fromAgent }, source);
  const history =
    session === null ? await fetchAllHistory(id, { agent: options?.fromAgent }, source) : undefined;
  return importSession(
    {
      agent,
      ...(session !== null ? { session } : {}),
      ...(history !== undefined ? { history } : {}),
      ...(options?.cwd !== undefined ? { cwd: options.cwd } : {}),
      ...(options?.title !== undefined ? { title: options.title } : {}),
    },
    target,
  );
}

export async function renameSession(
  id: string,
  title: string,
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const res = await sepiaFetch(
    `/api/sessions/${encodeURIComponent(id)}${agentQuery(agent)}`,
    {
      method: "PATCH",
      body: JSON.stringify({ title }),
    },
    target,
  );
  if (!res.ok) throw new Error(friendlyHttpError(res.status));
  return true;
}

export async function deleteSession(
  id: string,
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const res = await sepiaFetch(
    `/api/sessions/${encodeURIComponent(id)}${agentQuery(agent)}`,
    {
      method: "DELETE",
    },
    target,
  );
  if (!res.ok) throw new Error(friendlyHttpError(res.status));
  return true;
}

/**
 * `POST /api/sessions/:id/prompt` — `text` is the prompt body; `attachments`
 * is the ACP content-block array (`image`, `audio`, `resource`,
 * `resource_link` — built by `attachmentToPart` in lib/attachments.ts).
 */
export async function sendPrompt(
  id: string,
  input: { readonly text: string; readonly attachments?: ReadonlyArray<PromptPart> },
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const data = await request<{ ok: boolean }>(
    `/api/sessions/${encodeURIComponent(id)}/prompt${agentQuery(agent)}`,
    {
      method: "POST",
      body: JSON.stringify(
        input.attachments === undefined || input.attachments.length === 0
          ? { text: input.text }
          : { text: input.text, attachments: input.attachments },
      ),
    },
    target,
  );
  return data.ok;
}

export async function cancel(id: string, agent?: string, target?: ApiTarget): Promise<boolean> {
  const data = await request<{ ok: boolean }>(
    `/api/sessions/${encodeURIComponent(id)}/cancel${agentQuery(agent)}`,
    {
      method: "POST",
    },
    target,
  );
  return data.ok;
}

export async function respondToPermission(
  id: string,
  requestId: string,
  optionId: string | null,
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const data = await request<{ ok: boolean }>(
    `/api/sessions/${encodeURIComponent(id)}/permission${agentQuery(agent)}`,
    { method: "POST", body: JSON.stringify({ requestId, optionId }) },
    target,
  );
  return data.ok;
}

export interface AgUiEvent {
  type: string;
  name?: string;
  value?: unknown;
  [key: string]: unknown;
}

export type StreamStatus = "connecting" | "live" | "reconnecting";

export function subscribeSessionStream(
  id: string,
  onEvent: (event: AgUiEvent) => void,
  onStatus?: (status: StreamStatus) => void,
  agent?: string,
  target: ApiTarget = localTarget(),
): () => void {
  if (typeof EventSource === "undefined") return () => {};
  const params = new URLSearchParams();
  if (target.token !== null) params.set("access_token", target.token);
  if (agent !== undefined) params.set("agent", agent);
  const query = params.size === 0 ? "" : `?${params.toString()}`;
  const source = new EventSource(
    `${target.baseUrl}/api/sessions/${encodeURIComponent(id)}/stream${query}`,
  );
  onStatus?.("connecting");
  source.onopen = () => onStatus?.("live");
  // EventSource retries automatically; surface the gap instead of stalling silently.
  source.onerror = () => onStatus?.("reconnecting");
  source.onmessage = (message) => {
    try {
      onEvent(JSON.parse(message.data) as AgUiEvent);
    } catch {
      // Ignore non-JSON keep-alive frames.
    }
  };
  return () => source.close();
}
