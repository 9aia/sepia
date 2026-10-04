import type {
  AgentInfo,
  AttachResult,
  CreateSessionInput,
  HistoryPage,
  SessionSummary,
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

async function sepiaFetch(path: string, init?: RequestInit): Promise<Response> {
  const token = getToken();
  const res = await fetch(path, {
    ...init,
    headers: {
      ...(init?.body !== undefined ? { "content-type": "application/json" } : undefined),
      ...(token !== null ? { authorization: `Bearer ${token}` } : undefined),
    },
  });
  if (res.status === 401) throw new AuthError();
  return res;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await sepiaFetch(path, init);
  if (!res.ok) {
    throw new Error(`${init?.method ?? "GET"} ${path} failed: ${res.status}`);
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

export async function attach(id: string, options?: { takeover?: boolean }): Promise<AttachResult> {
  return request<AttachResult>(`/api/sessions/${encodeURIComponent(id)}/attach`, {
    method: "POST",
    body: options?.takeover === true ? JSON.stringify({ takeover: true }) : undefined,
  });
}

export async function listAgents(): Promise<AgentInfo[]> {
  const data = await request<{ agents: AgentInfo[] }>("/api/agents");
  return data.agents;
}

export async function deleteSession(id: string): Promise<boolean> {
  const res = await sepiaFetch(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (!res.ok) throw new Error(`DELETE /api/sessions/${id} failed: ${res.status}`);
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
