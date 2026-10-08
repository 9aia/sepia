import { Effect } from "effect";
import { describe, expect, it } from "vite-plus/test";
import type { ControlPlaneService, HistoryMessage } from "sepia-session-control";
import { ControlError } from "sepia-session-control";
import type { Event } from "sepia-agui";
import { EventType } from "sepia-agui";
import type { AcpCapabilities, PromptPart } from "sepia-acp";
import { createAguiAgentHandler } from "../src/agui-agent";

const CAPS: AcpCapabilities = {
  loadSession: true,
  sessionList: true,
  promptCapabilities: { image: true, audio: true, embeddedContext: true },
  sessionCapabilities: {
    list: true,
    delete: true,
    fork: false,
    resume: false,
    close: false,
    additionalDirectories: false,
  },
};

const HISTORY: ReadonlyArray<HistoryMessage> = [
  { role: "user", nodeId: 0, content: "first", createdAt: 1 },
  { role: "assistant", nodeId: 1, content: "reply", createdAt: 2 },
];

interface FakePlane {
  readonly plane: ControlPlaneService;
  readonly calls: {
    attach: Array<{ id: string; agentId?: string }>;
    subscribe: Array<{ id: string; agentId?: string }>;
    prompt: Array<{ id: string; parts: ReadonlyArray<PromptPart>; agentId?: string }>;
    history: Array<{ id: string; agentId?: string }>;
    cancel: string[];
  };
  readonly push: (id: string, events: ReadonlyArray<Event>) => void;
}

const makePlane = (over: Partial<ControlPlaneService> = {}): FakePlane => {
  const listeners = new Map<string, (events: ReadonlyArray<Event>) => void>();
  const calls: FakePlane["calls"] = {
    attach: [],
    subscribe: [],
    prompt: [],
    history: [],
    cancel: [],
  };
  const plane: ControlPlaneService = {
    listSessions: () => Effect.succeed([]),
    getHistory: (id, options) => {
      calls.history.push({ id, agentId: options?.agentId });
      return Effect.succeed({ messages: HISTORY, total: HISTORY.length, start: 0 });
    },
    getSession: () =>
      Effect.fail(new ControlError({ code: "not_found", message: "missing", cause: undefined })),
    getSummary: () =>
      Effect.fail(new ControlError({ code: "not_found", message: "missing", cause: undefined })),
    createSession: () => Effect.succeed({ id: "new", agentId: "devin", capabilities: CAPS }),
    attach: (id, options) => {
      calls.attach.push({ id, agentId: options?.agentId });
      return Effect.succeed({
        attached: true,
        readOnly: false,
        agentId: "devin",
        capabilities: CAPS,
      });
    },
    detach: () => Effect.void,
    prompt: (id, parts, agentId) => {
      calls.prompt.push({ id, parts, agentId });
      return Effect.void;
    },
    cancel: (id) => {
      calls.cancel.push(id);
      return Effect.void;
    },
    deleteSession: () => Effect.void,
    respondToPermission: () => Effect.void,
    restore: () => Effect.succeed({ restored: [], skipped: [] }),
    rewind: () => Effect.succeed({ kept: 0, removed: 0 }),
    subscribe: (id, listener, agentId) => {
      calls.subscribe.push({ id, agentId });
      listeners.set(id, listener);
      return Effect.succeed(() => listeners.delete(id));
    },
    listAgents: () => [],
    closeAll: () => Effect.void,
    ...over,
  };
  return { plane, calls, push: (id, events) => listeners.get(id)?.(events) };
};

const handler = (plane: ControlPlaneService, options = { keepAliveMs: 0 }) =>
  createAguiAgentHandler(plane, options);

const runInput = (body: unknown, query = ""): Request =>
  new Request(`http://localhost:8787/api/agent${query}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });

const BODY = {
  threadId: "t-1",
  runId: "r-1",
  forwardedProps: { sessionId: "sess-1" },
  messages: [{ role: "user", content: "hi" }],
};

describe("createAguiAgentHandler — request validation", () => {
  it("rejects a non-JSON body with 400", async () => {
    const { plane } = makePlane();
    const res = await handler(plane)(runInput("not json at all"));
    expect(res.status).toBe(400);
    await expect(res.json()).resolves.toEqual({ error: "Invalid JSON body" });
  });

  it("rejects a non-object body with 400", async () => {
    const { plane } = makePlane();
    for (const body of [[1, 2], 42, null]) {
      const res = await handler(plane)(runInput(body));
      expect(res.status).toBe(400);
      await expect(res.json()).resolves.toEqual({ error: "Expected a JSON object body" });
    }
  });

  it("rejects a missing session id with 400", async () => {
    const { plane } = makePlane();
    const res = await handler(plane)(runInput({ messages: BODY.messages }));
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("sessionId is required");
  });

  it("requires a non-empty user message", async () => {
    const { plane, calls } = makePlane();
    for (const messages of [
      [],
      [{ role: "assistant", content: "not user" }],
      [{ role: "user", content: "   " }],
      ["not-a-record", { role: "user", content: 42 }],
    ]) {
      const res = await handler(plane)(runInput({ forwardedProps: { sessionId: "s" }, messages }));
      expect(res.status).toBe(400);
      await expect(res.json()).resolves.toEqual({ error: "A user message is required" });
    }
    expect(calls.attach).toEqual([]);
  });
});

describe("createAguiAgentHandler — session id resolution", () => {
  it("reads the session id from ?sessionId=", async () => {
    const { plane, calls } = makePlane();
    const res = await handler(plane)(runInput(BODY, "?sessionId=qs-1"));
    expect(res.status).toBe(200);
    expect(calls.attach[0]?.id).toBe("qs-1");
    await res.body?.cancel();
  });

  it("reads it from forwardedProps or properties", async () => {
    const { plane, calls } = makePlane();
    await (await handler(plane)(runInput(BODY))).body?.cancel();
    await (
      await handler(plane)(
        runInput({ ...BODY, forwardedProps: undefined, properties: { sessionId: "prop-1" } }),
      )
    ).body?.cancel();
    expect(calls.attach.map((c) => c.id)).toEqual(["sess-1", "prop-1"]);
  });

  it("falls back to threadId as the session id", async () => {
    const { plane, calls } = makePlane();
    const res = await handler(plane)(
      runInput({ threadId: "thread-as-session", messages: BODY.messages }),
    );
    expect(res.status).toBe(200);
    expect(calls.attach[0]?.id).toBe("thread-as-session");
    await res.body?.cancel();
  });

  it("forwards the ?agent= disambiguator to the plane", async () => {
    const { plane, calls } = makePlane();
    const res = await handler(plane)(runInput(BODY, "?agent=cline"));
    expect(res.status).toBe(200);
    expect(calls.attach[0]?.agentId).toBe("cline");
    expect(calls.subscribe[0]?.agentId).toBe("cline");
    expect(calls.history[0]?.agentId).toBe("cline");
    await res.body?.cancel();
  });
});

describe("createAguiAgentHandler — plane failures", () => {
  it("maps an attach failure to 400", async () => {
    const { plane } = makePlane({
      attach: () =>
        Effect.fail(Object.assign(new Error("gone"), { _tag: "ControlError" }) as never),
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(400);
    await expect(res.json()).resolves.toEqual({ error: "gone" });
  });

  it("returns 409 when the session is locked by another process", async () => {
    const { plane } = makePlane({
      attach: () =>
        Effect.succeed({
          attached: false,
          readOnly: true,
          agentId: "devin",
          capabilities: CAPS,
        }),
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(409);
    const body = (await res.json()) as { error: string };
    expect(body.error).toContain("locked by another process");
  });

  it("maps a subscribe failure to 400", async () => {
    const { plane } = makePlane({
      subscribe: () => Effect.fail(Object.assign(new Error("no listener"), { _tag: "x" }) as never),
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(400);
    await expect(res.json()).resolves.toEqual({ error: "no listener" });
  });

  it("renders non-Error failure values via messageOf", async () => {
    const { plane } = makePlane();
    const handler = createAguiAgentHandler(plane, {
      keepAliveMs: 0,
      run: () => Promise.reject({ message: "plain object failure" }),
    });
    const res = await handler(runInput(BODY));
    expect(res.status).toBe(400);
    await expect(res.json()).resolves.toEqual({ error: "plain object failure" });
  });

  it("a synchronous busy rejection answers 409, not a stream", async () => {
    const { plane } = makePlane({
      prompt: () =>
        Effect.fail(
          Object.assign(new Error("Session is busy: sess-1"), {
            _tag: "ControlError",
            code: "busy",
          }) as never,
        ),
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(409);
    await expect(res.json()).resolves.toEqual({
      error: "Session is busy: sess-1",
      code: "busy",
    });
  });
});

describe("createAguiAgentHandler — streaming", () => {
  it("tags plane events with the request thread/run ids and finishes on RUN_FINISHED", async () => {
    const { plane, push } = makePlane();
    const res = await handler(plane)(runInput(BODY));
    expect(res.headers.get("content-type")).toContain("text/event-stream");

    const reader = res.body!.getReader();
    const decoder = new TextDecoder();
    push("sess-1", [{ type: EventType.RUN_STARTED } as Event]);
    const first = decoder.decode((await reader.read()).value);
    expect(first).toContain('"threadId":"t-1"');
    expect(first).toContain('"runId":"r-1"');

    // The buffered history snapshot flushes right after RUN_STARTED.
    const snapshot = decoder.decode((await reader.read()).value);
    expect(snapshot).toContain("MESSAGES_SNAPSHOT");
    expect(snapshot).toContain("sepia-history-0");

    push("sess-1", [{ type: EventType.RUN_FINISHED } as Event]);
    await reader.read();
    expect((await reader.read()).done).toBe(true);
  });

  it("uses the session id as threadId and mints a runId when absent", async () => {
    const { plane, push } = makePlane();
    const res = await handler(plane)(
      runInput({ forwardedProps: { sessionId: "sess-9" }, messages: BODY.messages }),
    );
    const reader = res.body!.getReader();
    push("sess-9", [{ type: EventType.RUN_STARTED } as Event]);
    const frame = new TextDecoder().decode((await reader.read()).value);
    expect(frame).toContain('"threadId":"sess-9"');
    expect(frame).toMatch(/"runId":"[0-9a-f-]{36}"/);
    await reader.cancel();
  });

  it("sends the last user text to the plane as the prompt", async () => {
    const { plane, calls } = makePlane();
    const res = await handler(plane)(
      runInput({
        ...BODY,
        messages: [
          { role: "user", content: "earlier" },
          { role: "assistant", content: "ack" },
          { role: "user", content: "the real prompt" },
        ],
      }),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(calls.prompt).toEqual([
      { id: "sess-1", parts: [{ type: "text", text: "the real prompt" }], agentId: undefined },
    ]);
    await res.body?.cancel();
  });
});

describe("createAguiAgentHandler — stream lifecycle edges", () => {
  it("rejects a body whose messages field is not an array", async () => {
    const { plane } = makePlane();
    const response = await handler(plane)(runInput({ ...BODY, messages: "not-a-list" }));
    expect(response.status).toBe(400);
    expect(((await response.json()) as { error: string }).error).toContain("user message");
  });

  it("swallows a cancel rejection and ignores events after termination", async () => {
    const { plane, calls, push } = makePlane();
    const recordAndFail = (id: string): never => {
      calls.cancel.push(id);
      // a failing cancel — the disconnect path must swallow it
      return Effect.fail(
        new ControlError({ code: "internal", message: "nope", cause: undefined }),
      ) as never;
    };
    (plane as { cancel: unknown }).cancel = (id: string) => recordAndFail(id);
    const ac = new AbortController();
    const request = new Request("http://localhost:8787/api/agent", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(BODY),
      signal: ac.signal,
    });
    const response = await handler(plane)(request);
    expect(response.status).toBe(200);
    // run ends, then the client aborts — the second finish() is a no-op and
    // cancel's rejection is swallowed
    push("sess-1", [{ type: EventType.RUN_FINISHED } as Event]);
    push("sess-1", [{ type: EventType.TEXT_MESSAGE_CONTENT } as Event]);
    ac.abort();
    await response.body?.cancel().catch(() => undefined);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(calls.cancel.length).toBeGreaterThan(0);
    expect(calls.cancel[0]).toBe("sess-1");
  });

  it("cancels the turn when the request was already aborted", async () => {
    const { plane, calls } = makePlane();
    const ac = new AbortController();
    ac.abort();
    const request = new Request("http://localhost:8787/api/agent", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(BODY),
      signal: ac.signal,
    });
    const response = await handler(plane)(request);
    // the abort listener fires on stream start — the turn is cancelled
    await response.body?.cancel().catch(() => undefined);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(calls.cancel).toEqual(["sess-1"]);
  });
});

describe("default options + terminal emit edges", () => {
  it("uses the default keepAlive when options are omitted", async () => {
    const { plane } = makePlane();
    const res = await createAguiAgentHandler(plane)(runInput(BODY));
    expect(res.status).toBe(200);
    await res.body?.cancel().catch(() => undefined);
  });

  it("messageOf renders a bare non-object failure", async () => {
    const { plane } = makePlane({
      attach: () => Effect.fail("plain-string-failure") as never,
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(400);
    expect(((await res.json()) as { error: string }).error).toBe("plain-string-failure");
  });

  it("a prompt rejecting after the client aborted hits the terminated-emit guard", async () => {
    const { plane, calls } = makePlane({
      prompt: () =>
        Effect.sleep("80 millis").pipe(
          Effect.andThen(
            Effect.fail(
              new ControlError({ code: "internal", message: "late failure", cause: undefined }),
            ),
          ),
        ),
    });
    const ac = new AbortController();
    const request = new Request("http://localhost:8787/api/agent", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(BODY),
      signal: ac.signal,
    });
    const res = await handler(plane)(request);
    expect(res.status).toBe(200);
    ac.abort(); // stream terminates before the prompt effect settles
    await new Promise((r) => setTimeout(r, 150));
    expect(calls.cancel.length).toBeGreaterThan(0);
  });
});

describe("messageOf + backlog edges", () => {
  it("renders a record-shaped failure's message field", async () => {
    const { plane } = makePlane();
    // inject a runner that rejects with the raw non-Error failure value
    const res = await createAguiAgentHandler(plane, {
      run: () => Promise.reject({ message: "structured but not Error" }),
    })(runInput(BODY));
    expect(res.status).toBe(400);
    expect(((await res.json()) as { error: string }).error).toBe("structured but not Error");
  });

  it("skips the snapshot when the backlog read fails", async () => {
    const { plane } = makePlane({
      getHistory: () =>
        Effect.fail(
          new ControlError({ code: "internal", message: "store gone", cause: undefined }),
        ),
    });
    const res = await handler(plane)(runInput(BODY));
    expect(res.status).toBe(200);
    await res.body?.cancel().catch(() => undefined);
  });
});
