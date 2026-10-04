import type {
  AgentInfo,
  AttachResult,
  CreateSessionInput,
  HistoryPage,
  SessionSummary,
} from "./types";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: init?.body ? { "content-type": "application/json" } : undefined,
    ...init,
  });
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

export async function getHistory(id: string, limit?: number): Promise<HistoryPage> {
  const query = limit === undefined ? "" : `?limit=${encodeURIComponent(String(limit))}`;
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
  const res = await fetch(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
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
  const source = new EventSource(`/api/sessions/${encodeURIComponent(id)}/stream`);
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
