import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Layer, Option } from "effect";
import { describe, expect, it } from "vite-plus/test";
import type { AcpConnection } from "sepia-acp";
import { SessionRepository } from "sepia-core";
import { ControlPlane, layer as controlPlaneLayer } from "sepia-session-control";
import type {
  AgentRuntime,
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

// GET /api/sessions always attaches the meta-overlay fields, meta store or not.
const SESSION_JSON = {
  ...SESSION,
  pinned: false,
  archived: false,
  projectIds: [],
  model: null,
  spans: [],
};

const HISTORY: ReadonlyArray<HistoryMessage> = [
  { role: "user", content: "hello", createdAt: 1 },
  { role: "assistant", content: "hi", createdAt: 2 },
];

// The real ControlError is a tagged error whose only fields the HTTP layer reads
// are `message` and `code`; a structural stand-in is enough for error mapping.
// sepia-core/session-control are still importable here thanks to the bun:sqlite
// stubs aliased in vitest.config.ts.
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
            return { id: "sess-new", agentId: options.agentId ?? "devin" };
          }),
    attach: (_id, options) =>
      Effect.succeed({
        attached: true,
        readOnly: options?.takeover !== true,
        agentId: options?.agentId ?? "devin",
      }),
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

const patch = (path: string, body: unknown): Request =>
  new Request(`http://localhost:8787${path}`, {
    method: "PATCH",
    headers: { origin: "http://localhost:3000", "content-type": "application/json" },
    body: JSON.stringify(body),
  });

const del = (path: string): Request =>
  new Request(`http://localhost:8787${path}`, {
    method: "DELETE",
    headers: { origin: "http://localhost:3000" },
  });

// Minimal ACP connection backing a real ControlPlane: enough for a live
// session that never flushes to the session store.
class StubConnection implements AcpConnection {
  readonly capabilities = { loadSession: true, sessionList: true };
  closed = false;
  async listSessions() {
    return [];
  }
  async newSession() {
    return "live-1";
  }
  async loadSession() {}
  async prompt() {}
  async cancel() {}
  async deleteSession() {}
  respondToPermission() {
    return true;
  }
  recentStderr() {
    return [];
  }
  onUpdate() {
    return () => {};
  }
  onPermission() {
    return () => {};
  }
  async close() {
    this.closed = true;
  }
}

const makeLivePlane = async (): Promise<ControlPlaneService> => {
  const repo = SessionRepository.of({
    save: () => Effect.void,
    getById: () => Effect.succeed(Option.none()),
    list: () => Effect.succeed([]),
    delete: () => Effect.void,
    hasSession: () => Effect.succeed(false),
  });
  const runtime: AgentRuntime = {
    id: "devin",
    label: "Devin",
    spawn: async () => new StubConnection(),
  };
  return Effect.runPromise(
    Effect.gen(function* () {
      return yield* ControlPlane;
    }).pipe(
      // idleTtlMs 0 disables the idle sweeper so no timer outlives the test.
      Effect.provide(controlPlaneLayer({ agents: [runtime], idleTtlMs: 0 })),
      Effect.provide(Layer.succeed(SessionRepository, repo)),
    ),
  );
};

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
    expect(response.headers.get("access-control-allow-methods")).toBe(
      "GET,POST,PATCH,DELETE,OPTIONS",
    );
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
    await expect(plain.json()).resolves.toEqual({ sessions: [SESSION_JSON] });

    const locked = await app(get("/api/sessions?withLocks=1"));
    await expect(locked.json()).resolves.toEqual({
      sessions: [{ ...SESSION_JSON, locked: true }],
    });
  });

  it("POST /api/sessions creates a session and returns 201 with the id", async () => {
    const { plane, created } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions", { cwd: "/tmp/sepia", title: "New" }),
    );

    expect(response.status).toBe(201);
    await expect(response.json()).resolves.toEqual({ id: "sess-new", agentId: "devin" });
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
    await expect(attached.json()).resolves.toEqual({
      attached: true,
      readOnly: true,
      agentId: "devin",
    });

    const taken = await app(post("/api/sessions/sess-1/attach", { takeover: true }));
    await expect(taken.json()).resolves.toEqual({
      attached: true,
      readOnly: false,
      agentId: "devin",
    });
  });

  it("POST /api/sessions/:id/attach records a run span in the meta overlay", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const meta = createMetaStore(join(dir, "meta.json"));
    const app = createApp(plane, {
      meta,
      node: { id: "node_test", name: "testbox", version: "0.0.0" },
    });

    const res = await app(post("/api/sessions/sess-1/attach?agent=cline", {}));
    expect(res.status).toBe(200);
    const spans = meta.of("sess-1")?.spans;
    expect(spans).toHaveLength(1);
    expect(spans?.[0]).toMatchObject({ agent: "cline", node: "node_test" });
    expect(typeof spans?.[0]?.at).toBe("number");

    // A same-agent re-attach on this node continues the same run — no dup.
    await app(post("/api/sessions/sess-1/attach?agent=cline", {}));
    expect(meta.of("sess-1")?.spans).toHaveLength(1);

    // Resuming under another agent opens a new span.
    await app(post("/api/sessions/sess-1/attach?agent=devin", {}));
    expect(meta.of("sess-1")?.spans?.map((s) => s.agent)).toEqual(["cline", "devin"]);

    // The spans ride out on the session list.
    const listed = await app(get("/api/sessions"));
    const body = (await listed.json()) as {
      sessions: Array<{ spans?: Array<unknown> }>;
    };
    expect(body.sessions[0]?.spans).toHaveLength(2);
  });

  it("a read-only attach records no span", async () => {
    const { plane } = makeFakePlane();
    const locked: ControlPlaneService = {
      ...plane,
      attach: () => Effect.succeed({ attached: false, readOnly: true, agentId: "devin" }),
    };
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const meta = createMetaStore(join(dir, "meta.json"));
    const app = createApp(locked, { meta });

    const res = await app(post("/api/sessions/sess-1/attach", {}));
    expect(res.status).toBe(200);
    expect(meta.of("sess-1")?.spans ?? []).toEqual([]);
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

  it("DELETE /api/sessions/:id returns ok even when the session is unknown", async () => {
    const { plane } = makeFakePlane();
    const ok = await createApp(plane)(
      new Request("http://localhost/api/sessions/sess-1", { method: "DELETE" }),
    );
    const missing = await createApp(plane)(
      new Request("http://localhost/api/sessions/missing", { method: "DELETE" }),
    );

    expect(ok.status).toBe(200);
    await expect(ok.json()).resolves.toEqual({ ok: true });
    // not_found is swallowed: created-but-unflushed sessions exist only in the
    // meta overlay, so deleting them must still succeed.
    expect(missing.status).toBe(200);
    await expect(missing.json()).resolves.toEqual({ ok: true });
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

  it("PATCH /api/sessions/:id archives and unarchives via meta", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const meta = createMetaStore(join(dir, "meta.json"));
    const app = createApp(plane, { meta });
    const res = await app(patch("/api/sessions/sess-1", { archived: true }));
    expect(res.status).toBe(200);
    expect(meta?.of("sess-1")?.archived).toBe(true);
    const listed = await app(get("/api/sessions"));
    const body = (await listed.json()) as {
      sessions: Array<{ id: string; archived: boolean }>;
    };
    expect(body.sessions.find((s) => s.id === "sess-1")?.archived).toBe(true);
    await app(patch("/api/sessions/sess-1", { archived: false }));
    expect(meta?.of("sess-1")?.archived).toBe(false);
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
    await expect(response.json()).resolves.toEqual({ sessions: [SESSION_JSON] });
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

  it("PATCH /api/config/:key writes through to the meta file; GET /api/config reads it back", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const metaPath = join(dir, "meta.json");
    const app = createApp(plane, { meta: createMetaStore(metaPath) });

    const patched = await app(patch("/api/config/ui.section.pinned", { value: false }));
    expect(patched.status).toBe(200);
    await expect(patched.json()).resolves.toEqual({
      key: "ui.section.pinned",
      value: false,
    });

    const read = await app(get("/api/config"));
    expect(read.status).toBe(200);
    await expect(read.json()).resolves.toEqual({
      config: { "ui.section.pinned": false },
    });

    // A fresh store over the same file sees the value — it is on disk, not in memory.
    const reloaded = await createApp(plane, { meta: createMetaStore(metaPath) })(
      get("/api/config"),
    );
    await expect(reloaded.json()).resolves.toEqual({
      config: { "ui.section.pinned": false },
    });
  });

  it("returns 501 for /api/config and /api/projects without a meta store", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane);

    expect((await app(get("/api/config"))).status).toBe(501);
    expect((await app(patch("/api/config/k", { value: 1 }))).status).toBe(501);
    expect((await app(get("/api/projects"))).status).toBe(501);
    expect((await app(post("/api/projects", { name: "x" }))).status).toBe(501);
  });

  it("POST/GET/PATCH/DELETE /api/projects manages the project list", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const app = createApp(plane, { meta: createMetaStore(join(dir, "meta.json")) });

    const created = await app(post("/api/projects", { name: "Alpha" }));
    expect(created.status).toBe(201);
    const { project } = (await created.json()) as {
      project: { id: string; name: string };
    };
    expect(project.name).toBe("Alpha");
    expect(project.id).toMatch(/^proj_/);

    const rejected = await app(post("/api/projects", { name: " " }));
    expect(rejected.status).toBe(400);

    const renamed = await app(patch(`/api/projects/${project.id}`, { name: "Beta" }));
    expect(renamed.status).toBe(200);

    const missing = await app(patch("/api/projects/proj-nope", { name: "X" }));
    expect(missing.status).toBe(404);

    const listed = await app(get("/api/projects"));
    await expect(listed.json()).resolves.toEqual({
      projects: [{ id: project.id, name: "Beta" }],
    });

    const deleted = await app(del(`/api/projects/${project.id}`));
    expect(deleted.status).toBe(200);
    await expect((await app(get("/api/projects"))).json()).resolves.toEqual({ projects: [] });
  });

  it("PATCH /api/sessions/:id stores pinned + projectIds and lands on GET /api/sessions", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const app = createApp(plane, { meta: createMetaStore(join(dir, "meta.json")) });

    const { project } = (await (await app(post("/api/projects", { name: "Work" }))).json()) as {
      project: { id: string };
    };

    const patched = await app(
      patch("/api/sessions/sess-1", { pinned: true, projectIds: [project.id] }),
    );
    expect(patched.status).toBe(200);

    const listed = await app(get("/api/sessions"));
    const { sessions } = (await listed.json()) as {
      sessions: Array<{ pinned?: boolean; projectIds?: string[] }>;
    };
    expect(sessions[0]?.pinned).toBe(true);
    expect(sessions[0]?.projectIds).toEqual([project.id]);

    // Deleting the project also strips its id from the session overlay.
    await app(del(`/api/projects/${project.id}`));
    const after = (await (await app(get("/api/sessions"))).json()) as {
      sessions: Array<{ projectIds?: string[] }>;
    };
    expect(after.sessions[0]?.projectIds).toEqual([]);

    const badIds = await app(patch("/api/sessions/sess-1", { projectIds: "x" }));
    expect(badIds.status).toBe(400);
  });

  it("GET /api/sessions/:id/history returns an empty page for a live, unflushed session", async () => {
    const plane = await makeLivePlane();
    const app = createApp(plane);
    try {
      // The agent keeps a fresh session live but only flushes it to the store
      // after the first prompt — history must be an empty page, not a 404.
      const created = await app(post("/api/sessions", { cwd: "/tmp/live" }));
      expect(created.status).toBe(201);
      const { id } = (await created.json()) as { id: string };
      expect(id).toBe("live-1");

      const history = await app(get(`/api/sessions/${id}/history`));
      expect(history.status).toBe(200);
      await expect(history.json()).resolves.toEqual({ messages: [], total: 0, start: 0 });

      // A true unknown still 404s.
      const unknown = await app(get("/api/sessions/ghost/history"));
      expect(unknown.status).toBe(404);
      await expect(unknown.json()).resolves.toEqual({
        error: "Unknown session: ghost",
        code: "not_found",
      });
    } finally {
      await Effect.runPromise(plane.closeAll());
    }
  });

  it("POST /api/sessions/:id/convert returns 501 when conversion is not configured", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions/sess-1/convert", { agent: "cline" }),
    );

    expect(response.status).toBe(501);
    await expect(response.json()).resolves.toEqual({
      error: "Convert is not configured on this server",
    });
  });

  it("POST /api/sessions/:id/convert rejects an unknown target agent with 400", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane, { convert: { dbPath: "/unused", clineDir: "/unused" } });

    const response = await app(post("/api/sessions/sess-1/convert", { agent: "cursor" }));
    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({
      error: "agent must be 'cline' or 'devin'",
    });
  });

  it("POST /api/sessions/:id/convert maps a store failure to 500 internal", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-convert-"));
    const app = createApp(plane, {
      convert: { dbPath: join(dir, "sessions.db"), clineDir: join(dir, "cline") },
    });

    // Under node the bun:sqlite stub makes the store layer fail to build; the
    // route must surface it as a ControlError, not an unhandled rejection.
    const response = await app(post("/api/sessions/sess-1/convert", { agent: "cline" }));
    expect(response.status).toBe(500);
    const body = (await response.json()) as { error: string; code?: string };
    expect(body.code).toBe("internal");
  });

  // The conversion happy path needs real bun:sqlite-backed stores (a Devin
  // sessions.db plus a Cline tasks dir); vitest aliases the driver to a
  // throwing stub under node. Cover it in tests/e2e.ts (runs under `bun`)
  // once a fixture store pair exists.
  it.todo("POST /api/sessions/:id/convert returns { sessionId } for a real store pair");

  const IMPORT_HISTORY = [
    { role: "system", content: "sys", createdAt: 1_000 },
    { role: "user", content: "hello", createdAt: 2_000 },
    { role: "assistant", content: "hi", createdAt: 3_000 },
    { role: "tool", content: "out", createdAt: 4_000, toolName: "exec" },
  ];

  it("POST /api/sessions/import returns 501 when conversion is not configured", async () => {
    const { plane } = makeFakePlane();
    const response = await createApp(plane)(
      post("/api/sessions/import", { agent: "cline", history: IMPORT_HISTORY }),
    );

    expect(response.status).toBe(501);
    await expect(response.json()).resolves.toEqual({
      error: "Import is not configured on this server",
    });
  });

  it("POST /api/sessions/import rejects a bad agent and malformed history", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane, {
      convert: { dbPath: "/unused", clineDir: "/unused" },
      importSession: () => Effect.succeed("never"),
    });

    const badAgent = await app(
      post("/api/sessions/import", { agent: "cursor", history: IMPORT_HISTORY }),
    );
    expect(badAgent.status).toBe(400);
    await expect(badAgent.json()).resolves.toEqual({
      error: "agent must be 'cline' or 'devin'",
    });

    const noHistory = await app(post("/api/sessions/import", { agent: "cline" }));
    expect(noHistory.status).toBe(400);

    const emptyHistory = await app(post("/api/sessions/import", { agent: "cline", history: [] }));
    expect(emptyHistory.status).toBe(400);

    const badItem = await app(
      post("/api/sessions/import", {
        agent: "cline",
        history: [{ role: "narrator", content: "x", createdAt: 1 }],
      }),
    );
    expect(badItem.status).toBe(400);

    const badCwd = await app(
      post("/api/sessions/import", { agent: "cline", cwd: "  ", history: IMPORT_HISTORY }),
    );
    expect(badCwd.status).toBe(400);
  });

  it("POST /api/sessions/import rebuilds the IR and returns the new session summary", async () => {
    const { plane } = makeFakePlane();
    const imported: Array<{ id: string; agent: string; nodes: number; promptHistory: number }> = [];
    const app = createApp(plane, {
      convert: { dbPath: "/unused", clineDir: "/unused" },
      importSession: (session, agent) =>
        Effect.sync(() => {
          imported.push({
            id: session.id,
            agent,
            nodes: session.nodes.length,
            promptHistory: session.promptHistory.length,
          });
          return "imported-1";
        }),
    });

    const response = await app(
      post("/api/sessions/import", {
        agent: "cline",
        cwd: "/home/dev/project",
        title: "Resumed work",
        history: IMPORT_HISTORY,
      }),
    );

    expect(response.status).toBe(201);
    const body = (await response.json()) as {
      id: string;
      title: string;
      cwd: string;
      agent: string;
      updatedAt: string;
    };
    expect(body).toMatchObject({
      id: "imported-1",
      title: "Resumed work",
      cwd: "/home/dev/project",
      agent: "cline",
    });
    expect(imported).toEqual([
      { id: expect.any(String), agent: "cline", nodes: 4, promptHistory: 1 },
    ]);
  });

  it("POST /api/sessions/import maps an executor failure to 500 internal", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane, {
      convert: { dbPath: "/unused", clineDir: "/unused" },
      importSession: () => Effect.fail(new Error("store exploded")),
    });

    const response = await app(
      post("/api/sessions/import", { agent: "devin", history: IMPORT_HISTORY }),
    );
    expect(response.status).toBe(500);
    const body = (await response.json()) as { error: string; code?: string };
    expect(body.code).toBe("internal");
  });
});

describe("push endpoints", () => {
  it("GET /api/push/vapid returns a stable public key", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-push-"));
    const app = createApp(plane, { meta: createMetaStore(join(dir, "meta.json")) });
    const first = await app(get("/api/push/vapid"));
    expect(first.status).toBe(200);
    const key = (await first.json()) as { publicKey: string };
    expect(typeof key.publicKey).toBe("string");
    expect(key.publicKey.length).toBeGreaterThan(20);
    // A second app over the same file reuses the generated key.
    const again = await createApp(plane, { meta: createMetaStore(join(dir, "meta.json")) })(
      get("/api/push/vapid"),
    );
    await expect(again.json()).resolves.toEqual(key);
  });

  it("POST/DELETE /api/push/subscribe stores and removes subscriptions", async () => {
    const { plane } = makeFakePlane();
    const dir = mkdtempSync(join(tmpdir(), "sepia-push-"));
    const meta = createMetaStore(join(dir, "meta.json"));
    const app = createApp(plane, { meta });
    const sub = {
      endpoint: "https://push.example/sub1",
      keys: { auth: "a", p256dh: "b" },
      prefs: { done: true, permission: false },
    };
    const post = await app(
      new Request("http://localhost/api/push/subscribe", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(sub),
      }),
    );
    expect(post.status).toBe(200);
    const stored = meta.config() as { pushSubscriptions?: unknown[] };
    expect(stored.pushSubscriptions).toHaveLength(1);
    const del = await app(
      new Request("http://localhost/api/push/subscribe", {
        method: "DELETE",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ endpoint: sub.endpoint }),
      }),
    );
    expect(del.status).toBe(200);
    expect((meta.config() as { pushSubscriptions?: unknown[] }).pushSubscriptions).toHaveLength(0);
  });

  it("returns 501 when no meta store is configured", async () => {
    const { plane } = makeFakePlane();
    const app = createApp(plane);
    const res = await app(get("/api/push/vapid"));
    expect(res.status).toBe(501);
  });
});
