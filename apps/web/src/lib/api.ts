import type {
  AgentInfo,
  AttachResult,
  CreateSessionInput,
  HistoryPage,
  SessionSummary,
  UserInfo,
} from "./types";

const TOKEN_KEY = "sepia:token";

export const getToken = (): string | null => {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
};

export const setToken = (token: string | null): void => {
  try {
    if (token === null || token === "") localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Storage unavailable (private mode); the gate keeps asking.
  }
};

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

async function sepiaFetch(path: string, init?: RequestInit): Promise<Response> {
  const token = getToken();
  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      headers: {
        ...(init?.body !== undefined ? { "content-type": "application/json" } : undefined),
        ...(token !== null ? { authorization: `Bearer ${token}` } : undefined),
      },
    });
  } catch {
    throw new Error("Can't reach the Sepia server — is it running?");
  }
  if (res.status === 401) throw new AuthError();
  return res;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await sepiaFetch(path, init);
  if (!res.ok) {
    throw new Error(friendlyHttpError(res.status));
  }
  return (await res.json()) as T;
}

export async function listSessions(): Promise<SessionSummary[]> {
  const data = await request<{ sessions: SessionSummary[] }>("/api/sessions");
  return data.sessions;
}

export async function createSession(input: CreateSessionInput): Promise<{ id: string }> {
  return request<{ id: string }>("/api/sessions", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function getHistory(
  id: string,
  options?: { limit?: number; before?: number },
): Promise<HistoryPage> {
  const params = new URLSearchParams();
  if (options?.limit !== undefined) params.set("limit", String(options.limit));
  if (options?.before !== undefined) params.set("before", String(options.before));
  const query = params.size === 0 ? "" : `?${params.toString()}`;
  return request<HistoryPage>(`/api/sessions/${encodeURIComponent(id)}/history${query}`);
}

export interface AttachOptions {
  readonly takeover?: boolean;
  readonly model?: string;
  readonly fallbacks?: ReadonlyArray<string>;
}

export async function attach(id: string, options?: AttachOptions): Promise<AttachResult> {
  const body: Record<string, unknown> = {};
  if (options?.takeover === true) body.takeover = true;
  if (options?.model !== undefined) body.model = options.model;
  if (options?.fallbacks !== undefined) body.fallbacks = options.fallbacks;
  return request<AttachResult>(`/api/sessions/${encodeURIComponent(id)}/attach`, {
    method: "POST",
    body: Object.keys(body).length === 0 ? undefined : JSON.stringify(body),
  });
}

export async function listDirs(path: string): Promise<string[]> {
  const data = await request<{ dirs: string[] }>(`/api/fs?path=${encodeURIComponent(path)}`);
  return data.dirs;
}

export async function getUserInfo(): Promise<UserInfo> {
  const data = await request<{ user: UserInfo }>("/api/user");
  return data.user;
}

export async function listAgents(): Promise<AgentInfo[]> {
  const data = await request<{ agents: AgentInfo[] }>("/api/agents");
  return data.agents;
}

export interface SessionMetaPatch {
  title?: string;
  pinned?: boolean;
  projectIds?: string[];
  model?: string | null;
}

export async function patchSessionMeta(id: string, patch: SessionMetaPatch): Promise<boolean> {
  const res = await sepiaFetch(`/api/sessions/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
  return res.ok;
}

export async function listProjects(): Promise<{ projects: import("./types").Project[] }> {
  return request(`/api/projects`);
}

export async function createProject(name: string): Promise<{ project: import("./types").Project }> {
  return request(`/api/projects`, { method: "POST", body: JSON.stringify({ name }) });
}

export async function renameProject(id: string, name: string): Promise<boolean> {
  const res = await sepiaFetch(`/api/projects/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ name }),
  });
  return res.ok;
}

export async function deleteProject(id: string): Promise<boolean> {
  const res = await sepiaFetch(`/api/projects/${encodeURIComponent(id)}`, { method: "DELETE" });
  return res.ok;
}

export async function convertSession(id: string, agent: string): Promise<{ sessionId: string }> {
  return request(`/api/sessions/${encodeURIComponent(id)}/convert`, {
    method: "POST",
    body: JSON.stringify({ agent }),
  });
}

export async function renameSession(id: string, title: string): Promise<boolean> {
  const res = await sepiaFetch(`/api/sessions/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ title }),
  });
  if (!res.ok) throw new Error(friendlyHttpError(res.status));
  return true;
}

export async function deleteSession(id: string): Promise<boolean> {
  const res = await sepiaFetch(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (!res.ok) throw new Error(friendlyHttpError(res.status));
  return true;
}

export async function sendPrompt(id: string, text: string): Promise<boolean> {
  const data = await request<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(id)}/prompt`, {
    method: "POST",
    body: JSON.stringify({ text }),
  });
  return data.ok;
}

export async function cancel(id: string): Promise<boolean> {
  const data = await request<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(id)}/cancel`, {
    method: "POST",
  });
  return data.ok;
}

export async function respondToPermission(
  id: string,
  requestId: string,
  optionId: string | null,
): Promise<boolean> {
  const data = await request<{ ok: boolean }>(
    `/api/sessions/${encodeURIComponent(id)}/permission`,
    { method: "POST", body: JSON.stringify({ requestId, optionId }) },
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
): () => void {
  if (typeof EventSource === "undefined") return () => {};
  const token = getToken();
  const query = token !== null ? `?access_token=${encodeURIComponent(token)}` : "";
  const source = new EventSource(`/api/sessions/${encodeURIComponent(id)}/stream${query}`);
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
