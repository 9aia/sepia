import type {
  AgentInfo,
  AgentKind,
  AttachResult,
  CreateSessionInput,
  HistoryMessage,
  HistoryPage,
  SessionSummary,
} from "./types";

// Mock data is a dev-only affordance; a production build never fakes the API.
const MOCK = import.meta.env.DEV && import.meta.env.VITE_MOCK === "1";

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

const MOCK_SESSIONS: SessionSummary[] = [
  {
    id: "sess-1",
    title: "Refactor auth middleware",
    cwd: "/home/dev/projects/api-server",
    agent: "devin",
    updatedAt: new Date(Date.now() - 1000 * 60 * 3).toISOString(),
    locked: true,
    lockHolderPid: 48213,
    source: "devin",
    busy: false,
  },
  {
    id: "sess-2",
    title: "Fix flaky billing tests",
    cwd: "/home/dev/projects/billing",
    agent: "cline",
    updatedAt: new Date(Date.now() - 1000 * 60 * 42).toISOString(),
    locked: false,
    lockHolderPid: null,
    source: "cline",
    busy: false,
  },
  {
    id: "sess-3",
    title: "Add session streaming endpoint",
    cwd: "/home/dev/projects/sepia/apps/server",
    agent: "devin",
    updatedAt: new Date(Date.now() - 1000 * 60 * 60 * 5).toISOString(),
    locked: false,
    lockHolderPid: null,
    source: "devin",
    busy: false,
  },
];

const MOCK_HISTORY: Record<string, HistoryMessage[]> = {
  "sess-1": [
    {
      role: "user",
      content: "Why does the auth middleware reject valid tokens?",
      createdAt: Date.now() - 600000,
    },
    {
      role: "assistant",
      content:
        "The clock skew check uses a 30s window; your tokens expire faster than that on refresh.",
      createdAt: Date.now() - 540000,
    },
    {
      role: "tool",
      content: "read_file src/auth/middleware.ts",
      createdAt: Date.now() - 500000,
      toolName: "read_file",
    },
  ],
  "sess-2": [
    { role: "system", content: "Session resumed.", createdAt: Date.now() - 900000 },
    {
      role: "user",
      content: "The billing tests fail intermittently in CI.",
      createdAt: Date.now() - 800000,
    },
  ],
};

export async function listSessions(): Promise<SessionSummary[]> {
  if (MOCK) return MOCK_SESSIONS;
  const data = await request<{ sessions: SessionSummary[] }>("/api/sessions");
  return data.sessions;
}

export async function createSession(input: CreateSessionInput): Promise<{ id: string }> {
  if (MOCK) {
    const id = `sess-${MOCK_SESSIONS.length + 1}`;
    MOCK_SESSIONS.push({
      id,
      title: input.title ?? "New session",
      cwd: input.cwd,
      agent: (input.agent as AgentKind | undefined) ?? "devin",
      updatedAt: new Date().toISOString(),
      locked: false,
      lockHolderPid: null,
      source: "sepia",
      busy: false,
    });
    return { id };
  }
  return request<{ id: string }>("/api/sessions", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function getHistory(id: string, limit?: number): Promise<HistoryPage> {
  if (MOCK) {
    const messages = MOCK_HISTORY[id] ?? [];
    return { messages, total: messages.length };
  }
  const query = limit === undefined ? "" : `?limit=${encodeURIComponent(String(limit))}`;
  return request<HistoryPage>(`/api/sessions/${encodeURIComponent(id)}/history${query}`);
}

export async function attach(id: string, options?: { takeover?: boolean }): Promise<AttachResult> {
  if (MOCK) {
    const session = MOCK_SESSIONS.find((s) => s.id === id);
    return {
      attached: true,
      readOnly: options?.takeover === true ? false : (session?.locked ?? false),
    };
  }
  return request<AttachResult>(`/api/sessions/${encodeURIComponent(id)}/attach`, {
    method: "POST",
    body: options?.takeover === true ? JSON.stringify({ takeover: true }) : undefined,
  });
}

export async function listAgents(): Promise<AgentInfo[]> {
  if (MOCK) return [{ id: "devin", label: "Devin" }];
  const data = await request<{ agents: AgentInfo[] }>("/api/agents");
  return data.agents;
}

export async function deleteSession(id: string): Promise<boolean> {
  if (MOCK) {
    const index = MOCK_SESSIONS.findIndex((s) => s.id === id);
    if (index !== -1) MOCK_SESSIONS.splice(index, 1);
    return true;
  }
  const res = await fetch(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (!res.ok) throw new Error(`DELETE /api/sessions/${id} failed: ${res.status}`);
  return true;
}

export async function sendPrompt(id: string, text: string): Promise<boolean> {
  if (MOCK) return true;
  const data = await request<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(id)}/prompt`, {
    method: "POST",
    body: JSON.stringify({ text }),
  });
  return data.ok;
}

export async function cancel(id: string): Promise<boolean> {
  if (MOCK) return true;
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
  if (MOCK) return true;
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
  if (MOCK || typeof EventSource === "undefined") return () => {};
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

export const isMock = MOCK;
