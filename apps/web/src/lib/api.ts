import type {
  AgentInfo,
  AttachResult,
  CreateSessionInput,
  HistoryMessage,
  HistoryPage,
  NodeDescriptor,
  SessionSummary,
  UserInfo,
} from "./types";
import { localTarget, type ApiTarget } from "./targets";

export { getToken, setToken } from "./token";
export type { ApiTarget } from "./targets";

export class AuthError extends Error {
  constructor() {
    super("Unauthorized");
    this.name = "AuthError";
  }
}

const friendlyHttpError = (status: number): string => {
  if (status === 400) return "The server rejected the request";
  if (status === 403) return "Access denied";
  if (status === 404) return "Not found";
  if (status === 409) return "That operation is busy — try again in a moment";
  if (status >= 500) return "The server hit an error — try again";
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
    throw new Error("Can't reach the Sepia server — is it running?");
  }
  if (res.status === 401) throw new AuthError();
  return res;
}

async function request<T>(path: string, init?: RequestInit, target?: ApiTarget): Promise<T> {
  const res = await sepiaFetch(path, init, target);
  if (!res.ok) {
    throw new Error(friendlyHttpError(res.status));
  }
  return (await res.json()) as T;
}

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
export async function pairNode(code: string, target: ApiTarget): Promise<{ token: string }> {
  const res = await sepiaFetch(
    "/api/pair",
    { method: "POST", body: JSON.stringify({ code }) },
    // Pairing is pre-credential — never send a stored token along.
    { ...target, token: null },
  );
  if (res.status === 404) {
    throw new Error("That code didn't work — it may have expired or already been used");
  }
  if (!res.ok) {
    throw new Error(friendlyHttpError(res.status));
  }
  return (await res.json()) as { token: string };
}

export async function listSessions(target?: ApiTarget): Promise<SessionSummary[]> {
  const data = await request<{ sessions: SessionSummary[] }>("/api/sessions", undefined, target);
  return data.sessions;
}

export async function createSession(
  input: CreateSessionInput,
  target?: ApiTarget,
): Promise<{ id: string; agentId?: string }> {
  return request<{ id: string; agentId?: string }>(
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

export interface ImportSessionInput {
  readonly agent: string;
  readonly cwd?: string;
  readonly title?: string;
  readonly history: ReadonlyArray<HistoryMessage>;
}

/** POST /api/sessions/import — write explicit IR history into an agent's store. */
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
 * "Resume on…": pull the full IR history off the source node, then replay it
 * into `agent`'s store on the target node via POST /api/sessions/import.
 * Same node + different agent is the same-machine agent switch; a peer
 * target moves the session to another machine.
 */
export async function resumeSession(
  source: ApiTarget | undefined,
  id: string,
  agent: string,
  target: ApiTarget | undefined,
  options?: ResumeOptions,
): Promise<SessionSummary> {
  const history = await fetchAllHistory(id, { agent: options?.fromAgent }, source);
  return importSession(
    {
      agent,
      history,
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

export async function sendPrompt(
  id: string,
  text: string,
  agent?: string,
  target?: ApiTarget,
): Promise<boolean> {
  const data = await request<{ ok: boolean }>(
    `/api/sessions/${encodeURIComponent(id)}/prompt${agentQuery(agent)}`,
    {
      method: "POST",
      body: JSON.stringify({ text }),
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
