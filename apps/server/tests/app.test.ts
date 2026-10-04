import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { describe, expect, it } from "vite-plus/test";
import type {
  ControlError,
  ControlErrorCode,
  ControlPlaneService,
  HistoryMessage,
  SessionEventListener,
  SessionSummary,
} from "sepia-session-control";
import type { Event } from "sepia-agui";
import { EventType } from "sepia-agui";
import { createApp } from "../src/app";
import { createMetaStore } from "../src/meta";

const SESSION: SessionSummary = {
  id: "sess-1",
  title: "Refactor auth middleware",
  cwd: "/home/dev/projects/api-server",
  agent: "devin",
  updatedAt: new Date(0).toISOString(),
  locked: false,
  lockHolderPid: null,
  source: "devin",
  busy: false,
};

const HISTORY: ReadonlyArray<HistoryMessage> = [
  { role: "user", content: "hello", createdAt: 1 },
  { role: "assistant", content: "hi", createdAt: 2 },
];

// The real ControlError is a tagged error whose only fields the HTTP layer reads
// are `message` and `code`; a structural stand-in keeps the vitest module graph
// free of the bun-only sqlite store that `sepia-session-control` pulls in.
const failure = (message: string, code?: ControlErrorCode): Effect.Effect<never, ControlError> =>
  Effect.fail(
    Object.assign(new Error(message), {
      _tag: "ControlError",
      code,
      cause: undefined,
    }) as ControlError,
  );

interface FakePlane {
  readonly plane: ControlPlaneService;
  readonly push: (id: string, events: ReadonlyArray<Event>) => void;
  readonly prompts: Array<{ readonly id: string; readonly text: string }>;
  readonly cancels: string[];
  readonly permissions: Array<{
    readonly id: string;
    readonly requestId: string;
    readonly optionId: string | null;
  }>;
  readonly created: Array<{
    readonly cwd: string;
    readonly agentId?: string;
    readonly title?: string;
  }>;
}

const makeFakePlane = (): FakePlane => {
  const listeners = new Map<string, SessionEventListener>();
  const prompts: Array<{ id: string; text: string }> = [];
  const cancels: string[] = [];
  const permissions: Array<{ id: string; requestId: string; optionId: string | null }> = [];
  const created: Array<{ cwd: string; agentId?: string; title?: string }> = [];

  const plane: ControlPlaneService = {
    listSessions: (options) =>
      Effect.succeed(options?.withLocks === true ? [{ ...SESSION, locked: true }] : [SESSION]),
    getHistory: (id) =>
      id === "missing"
        ? failure("session not found: missing", "not_found")
        : Effect.succeed({ messages: HISTORY, total: HISTORY.length, start: 0 }),
    createSession: (options) =>
      options.agentId === "bad"
        ? failure("Unknown agent: bad", "unknown_agent")
        : Effect.sync(() => {
            created.push(options);
            return { id: "sess-new" };
          }),
    attach: (_id, options) =>
      Effect.succeed({ attached: true, readOnly: options?.takeover !== true }),
    detach: () => Effect.void,
    prompt: (id, text) =>
      Effect.sync(() => {
        prompts.push({ id, text });
      }),
    cancel: (id) =>
      Effect.sync(() => {
        cancels.push(id);
      }),
    deleteSession: (id) =>
      id === "missing"
        ? failure("session not found: missing", "not_found")
        : Effect.sync(() => {
            cancels.push(id);
          }),
    respondToPermission: (id, requestId, optionId) =>
      optionId === "bad"
        ? failure("invalid option id", "invalid")
        : Effect.sync(() => {
            permissions.push({ id, requestId, optionId });
          }),
    subscribe: (id, listener) =>
      Effect.sync(() => {
        listeners.set(id, listener);
        return () => {
          listeners.delete(id);
        };
      }),
    listAgents: () => [
      { id: "devin", label: "Devin" },
      { id: "cline", label: "Cline" },
    ],
    closeAll: () => Effect.void,
  };

  return {
    plane,
    prompts,
    cancels,
    permissions,
    created,
    push: (id, events) => listeners.get(id)?.(events),
  };
};

const get = (path: string, origin = "http://localhost:3000"): Request =>
  new Request(`http://localhost:8787${path}`, { headers: { origin } });

const post = (path: string, body?: unknown): Request =>
  new Request(`http://localhost:8787${path}`, {
    method: "POST",
    headers: {
      origin: "http://localhost:3000",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });

describe("createApp", () => {
  it("answers OPTIONS preflight with 204 and CORS headers", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      new Request("http://localhost:8787/api/sessions", {
        method: "OPTIONS",
        headers: { origin: "http://localhost:3000" },
      }),
    );

    expect(response.status).toBe(204);
    expect(response.headers.get("access-control-allow-origin")).toBe("http://localhost:3000");
    expect(response.headers.get("access-control-allow-methods")).toBe("GET,POST,OPTIONS");
    expect(response.headers.get("access-control-allow-headers")).toBe("content-type,authorization");
  });

  it("honors a custom origin allowlist", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { allowedOrigins: ["https://sepia.example"] })(
      get("/api/sessions", "https://sepia.example"),
    );

    expect(response.headers.get("access-control-allow-origin")).toBe("https://sepia.example");
  });

  it("GET /api/agents returns the agent list", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(get("/api/agents"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({
      agents: [
        { id: "devin", label: "Devin" },
        { id: "cline", label: "Cline" },
      ],
    });
  });

  it("GET /api/sessions returns summaries and honors withLocks", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane);

    const plain = await app(get("/api/sessions"));
    expect(plain.status).toBe(200);
    await expect(plain.json()).resolves.toEqual({ sessions: [SESSION] });

    const locked = await app(get("/api/sessions?withLocks=1"));
    await expect(locked.json()).resolves.toEqual({ sessions: [{ ...SESSION, locked: true }] });
  });

  it("POST /api/sessions creates a session and returns 201 with the id", async () => {
    const { plane, created } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions", { cwd: "/tmp/sepia", title: "New" }),
    );

    expect(response.status).toBe(201);
    await expect(response.json()).resolves.toEqual({ id: "sess-new" });
    expect(created).toEqual([{ cwd: "/tmp/sepia", title: "New" }]);
  });

  it("rejects a create without cwd with 400", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(post("/api/sessions", { title: "no cwd" }));

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({ error: "cwd is required" });
  });

  it("maps an unknown agent on create to 400 with a code", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions", { cwd: "/tmp/sepia", agent: "bad" }),
    );

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: "Unknown agent: bad",
      code: "unknown_agent",
    });
  });

  it("GET /api/sessions/:id/history returns messages and total", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(get("/api/sessions/sess-1/history"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({
      messages: HISTORY,
      total: HISTORY.length,
      start: 0,
    });
  });

  it("forwards ?limit to getHistory", async () => {
    const { plane } = makeFakePlane();
    const seen: Array<number | undefined> = [];
    const tracking: ControlPlaneService = {
      ...plane,
      getHistory: (_id, options) => {
        seen.push(options?.limit);
        return Effect.succeed({ messages: HISTORY, total: 1500, start: 0 });
      },
    };
    const response = await createApp(tracking)(get("/api/sessions/sess-1/history?limit=5"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ messages: HISTORY, total: 1500, start: 0 });
    expect(seen).toEqual([5]);
  });

  it("rejects a non-integer limit with 400", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(get("/api/sessions/sess-1/history?limit=abc"));

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: "limit must be a non-negative integer",
    });
  });

  it("maps a missing session to 404 with a code", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(get("/api/sessions/missing/history"));

    expect(response.status).toBe(404);
    await expect(response.json()).resolves.toEqual({
      error: "session not found: missing",
      code: "not_found",
    });
  });

  it("maps a ControlError code to an HTTP status", async () => {
    const { plane } = makeFakePlane();
    const locked: ControlPlaneService = {
      ...plane,
      getHistory: () => failure("locked by another process", "locked"),
    };
    const response = await createApp(locked)(get("/api/sessions/sess-1/history"));

    expect(response.status).toBe(409);
    await expect(response.json()).resolves.toEqual({
      error: "locked by another process",
      code: "locked",
    });
  });

  it("maps a busy ControlError to 409", async () => {
    const { plane } = makeFakePlane();
    const busy: ControlPlaneService = {
      ...plane,
      prompt: () => failure("Session is busy: sess-1", "busy"),
    };
    const response = await createApp(busy)(post("/api/sessions/sess-1/prompt", { text: "go" }));

    expect(response.status).toBe(409);
    await expect(response.json()).resolves.toEqual({
      error: "Session is busy: sess-1",
      code: "busy",
    });
  });

  it("falls back to 500 for an error without a code", async () => {
    const { plane } = makeFakePlane();
    const boom: ControlPlaneService = { ...plane, getHistory: () => failure("boom") };
    const response = await createApp(boom)(get("/api/sessions/sess-1/history"));

    expect(response.status).toBe(500);
    await expect(response.json()).resolves.toEqual({ error: "boom" });
  });

  it("POST /api/sessions/:id/attach returns AttachResult", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane);

    const attached = await app(post("/api/sessions/sess-1/attach", {}));
    expect(attached.status).toBe(200);
    await expect(attached.json()).resolves.toEqual({ attached: true, readOnly: true });

    const taken = await app(post("/api/sessions/sess-1/attach", { takeover: true }));
    await expect(taken.json()).resolves.toEqual({ attached: true, readOnly: false });
  });

  it("POST /api/sessions/:id/prompt forwards text and returns ok", async () => {
    const { plane, prompts } = makeFakePlane();
    const response = await createApp(plane)(post("/api/sessions/sess-1/prompt", { text: "go" }));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ ok: true });
    expect(prompts).toEqual([{ id: "sess-1", text: "go" }]);
  });

  it("rejects an empty prompt with 400", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(post("/api/sessions/sess-1/prompt", { text: "  " }));

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({ error: "text is required" });
  });

  it("POST /api/sessions/:id/cancel returns ok", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(post("/api/sessions/sess-1/cancel"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ ok: true });
  });

  it("DELETE /api/sessions/:id returns ok and maps missing sessions to 404", async () => {
    const { plane } = makeFakePlane();
    const ok = await createApp(plane)(
      new Request("http://localhost/api/sessions/sess-1", { method: "DELETE" }),
    );
    const missing = await createApp(plane)(
      new Request("http://localhost/api/sessions/missing", { method: "DELETE" }),
    );

    expect(ok.status).toBe(200);
    await expect(ok.json()).resolves.toEqual({ ok: true });
    expect(missing.status).toBe(404);
  });

  it("PATCH /api/sessions/:id renames via the meta overlay and overlays on list", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const meta = createMetaStore(join(dir, "meta.json"));
    const app = createApp(plane, { meta });

    const renamed = await app(
      new Request("http://localhost/api/sessions/sess-1", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ title: "Renamed session" }),
      }),
    );
    expect(renamed.status).toBe(200);

    const list = await app(get("/api/sessions"));
    const { sessions } = (await list.json()) as { sessions: SessionSummary[] };
    expect(sessions[0]?.title).toBe("Renamed session");

    const invalid = await app(
      new Request("http://localhost/api/sessions/sess-1", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ title: " " }),
      }),
    );
    expect(invalid.status).toBe(400);

    const unconfigured = await createApp(plane)(
      new Request("http://localhost/api/sessions/sess-1", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ title: "x" }),
      }),
    );
    expect(unconfigured.status).toBe(501);
  });

  it("POST /api/sessions/:id/permission forwards the decision", async () => {
    const { plane, permissions } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions/sess-1/permission", { requestId: "req-1", optionId: "allow" }),
    );

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ ok: true });
    expect(permissions).toEqual([{ id: "sess-1", requestId: "req-1", optionId: "allow" }]);
  });

  it("maps an invalid permission option to 400 with a code", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions/sess-1/permission", { requestId: "req-1", optionId: "bad" }),
    );

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: "invalid option id",
      code: "invalid",
    });
  });

  it("GET /api/sessions/:id/stream streams SSE frames", async () => {
    const { plane, push } = makeFakePlane();
    const response = await createApp(plane)(get("/api/sessions/sess-1/stream"));

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/event-stream");

    const reader = response.body?.getReader();
    expect(reader).toBeDefined();

    push("sess-1", [
      { type: "TEXT_MESSAGE_CONTENT", messageId: "m1", delta: "hello" } as unknown as Event,
    ]);

    const { value } = (await reader?.read()) ?? {};
    const frame = new TextDecoder().decode(value);
    expect(frame.startsWith("data: ")).toBe(true);
    expect(frame).toContain("TEXT_MESSAGE_CONTENT");
    expect(frame.endsWith("\n\n")).toBe(true);

    await reader?.cancel();
  });

  it("emits SSE keep-alive comment frames", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { keepAliveMs: 5 })(get("/api/sessions/sess-1/stream"));

    const reader = response.body!.getReader();
    const { value } = await reader.read();
    expect(new TextDecoder().decode(value)).toBe(": ping\n\n");
    await reader.cancel();
  });

  it("returns 404 for unknown routes", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(get("/api/nope"));

    expect(response.status).toBe(404);
    await expect(response.json()).resolves.toEqual({ error: "Not found" });
  });

  it("POST /api/agent streams AG-UI events re-tagged with the request ids", async () => {
    const { plane, push, prompts } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/agent", {
        threadId: "t-1",
        runId: "r-1",
        forwardedProps: { sessionId: "sess-1" },
        messages: [{ id: "m1", role: "user", content: "hi" }],
      }),
    );

    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("text/event-stream");

    const reader = response.body!.getReader();
    const decoder = new TextDecoder();

    push("sess-1", [{ type: EventType.RUN_STARTED, threadId: "x", runId: "y" } as Event]);
    const first = decoder.decode((await reader.read()).value);
    // The run opens before the snapshot: clients allocate messages[] on
    // RUN_STARTED, so an earlier snapshot would apply to undefined state.
    expect(first).toContain("RUN_STARTED");
    expect(first).toContain('"threadId":"t-1"');
    expect(first).toContain('"runId":"r-1"');

    const snapshot = decoder.decode((await reader.read()).value);
    expect(snapshot).toContain("MESSAGES_SNAPSHOT");
    expect(snapshot).toContain('"role":"assistant"');

    push("sess-1", [{ type: EventType.RUN_FINISHED, threadId: "x", runId: "y" } as Event]);
    await reader.read();

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(prompts).toEqual([{ id: "sess-1", text: "hi" }]);
  });

  it("delivers an event pushed during the subscribe window", async () => {
    const { plane } = makeFakePlane();
    const withInitial: ControlPlaneService = {
      ...plane,
      subscribe: (_id, listener) =>
        Effect.sync(() => {
          listener([{ type: EventType.RUN_STARTED, threadId: "x", runId: "y" } as Event]);
          return () => {};
        }),
    };
    const response = await createApp(withInitial)(
      post("/api/agent", {
        threadId: "t-1",
        runId: "r-1",
        forwardedProps: { sessionId: "sess-1" },
        messages: [{ id: "m1", role: "user", content: "hi" }],
      }),
    );

    const reader = response.body!.getReader();
    const frame = new TextDecoder().decode((await reader.read()).value);
    expect(frame).toContain("RUN_STARTED");
    expect(frame).toContain('"threadId":"t-1"');
    await reader.cancel();
  });

  it("cancels the turn when the client disconnects", async () => {
    const { plane, cancels } = makeFakePlane();
    const abort = new AbortController();
    const request = new Request("http://localhost:8787/api/agent?sessionId=sess-1", {
      method: "POST",
      headers: { origin: "http://localhost:3000", "content-type": "application/json" },
      body: JSON.stringify({
        threadId: "t-1",
        runId: "r-1",
        messages: [{ id: "m1", role: "user", content: "hi" }],
      }),
      signal: abort.signal,
    });

    const response = await createApp(plane)(request);
    expect(response.status).toBe(200);

    abort.abort();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(cancels).toEqual(["sess-1"]);
  });

  it("returns 409 when a second /api/agent turn is already running", async () => {
    const { plane } = makeFakePlane();
    let busy = false;
    let release: () => void = () => {};
    const gated: ControlPlaneService = {
      ...plane,
      prompt: () => {
        if (busy) return failure("Session is busy: sess-1", "busy");
        busy = true;
        return Effect.promise(
          () =>
            new Promise<void>((resolve) => {
              release = () => {
                busy = false;
                resolve();
              };
            }),
        );
      },
    };
    const app = createApp(gated);
    const body = {
      threadId: "t-1",
      runId: "r-1",
      forwardedProps: { sessionId: "sess-1" },
      messages: [{ id: "m1", role: "user", content: "hi" }],
    };

    const first = await app(post("/api/agent", body));
    expect(first.status).toBe(200);

    const second = await app(post("/api/agent", body));
    expect(second.status).toBe(409);
    await expect(second.json()).resolves.toEqual({
      error: "Session is busy: sess-1",
      code: "busy",
    });

    release();
    await first.body?.cancel();
  });

  it("POST /api/agent rejects an input without a session id", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/agent", { threadId: "", messages: [{ role: "user", content: "hi" }] }),
    );

    expect(response.status).toBe(400);
    const body = (await response.json()) as { error: string };
    expect(body.error).toContain("sessionId is required");
  });

  const authed = (path: string, token: string): Request =>
    new Request(`http://localhost:8787${path}`, {
      headers: { origin: "http://localhost:3000", authorization: `Bearer ${token}` },
    });

  it("rejects a /api request without a token with 401", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { token: "secret" })(get("/api/sessions"));

    expect(response.status).toBe(401);
    expect(response.headers.get("www-authenticate")).toBe("Bearer");
    await expect(response.json()).resolves.toEqual({ error: "Unauthorized" });
  });

  it("rejects a wrong bearer token with 401", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { token: "secret" })(authed("/api/sessions", "wrong"));

    expect(response.status).toBe(401);
    await expect(response.json()).resolves.toEqual({ error: "Unauthorized" });
  });

  it("accepts the correct bearer token", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { token: "secret" })(authed("/api/sessions", "secret"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ sessions: [SESSION] });
  });

  it("serves a healthy GET /api/health without a token", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane, { token: "secret" })(get("/api/health"));

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ ok: true, db: true });
  });

  it("reports 503 when the session store is unreadable", async () => {
    const { plane } = makeFakePlane();
    const broken: ControlPlaneService = {
      ...plane,
      listSessions: () => failure("database is locked", "internal"),
    };
    const response = await createApp(broken)(get("/api/health"));

    expect(response.status).toBe(503);
    await expect(response.json()).resolves.toEqual({ ok: false, db: false });
  });

  it("logs one line per request and suppresses /api/health", async () => {
    const { plane } = makeFakePlane();
    const lines: string[] = [];
    const app = createApp(plane, { logger: (line) => lines.push(line) });

    await app(get("/api/sessions"));
    await app(get("/api/health"));

    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(/^GET \/api\/sessions 200 \d+ms$/);
  });

  it("rejects an unauthenticated create without touching the plane", async () => {
    const { plane, created } = makeFakePlane();
    const response = await createApp(plane, { token: "secret" })(
      post("/api/sessions", { cwd: "/tmp/sepia" }),
    );

    expect(response.status).toBe(401);
    expect(created).toEqual([]);
  });

  it("POST /api/agent emits RUN_ERROR when the prompt rejects", async () => {
    const { plane } = makeFakePlane();
    const failingPlane: ControlPlaneService = {
      ...plane,
      prompt: () => failure("agent process exited"),
    };
    const response = await createApp(failingPlane)(
      post("/api/agent", {
        threadId: "t-1",
        runId: "r-1",
        forwardedProps: { sessionId: "sess-1" },
        messages: [{ id: "m1", role: "user", content: "hi" }],
      }),
    );

    expect(response.status).toBe(200);
    const reader = response.body!.getReader();
    // The prompt rejects before RUN_STARTED, so the buffered snapshot never
    // flushes; the first frame is the run error.
    const frame = new TextDecoder().decode((await reader.read()).value);
    expect(frame).toContain("RUN_ERROR");
    expect(frame).toContain("agent process exited");
  });
});
