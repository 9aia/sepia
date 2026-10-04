import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  attach,
  AuthError,
  cancel,
  convertSession,
  createProject,
  createSession,
  deleteProject,
  deleteSession,
  getConfig,
  getHistory,
  getNode,
  getUserInfo,
  listAgents,
  listDirs,
  listProjects,
  listSessions,
  pairNode,
  patchSessionMeta,
  renameProject,
  renameSession,
  respondToPermission,
  resumeSession,
  sendPrompt,
  setConfigKey,
  setToken,
  subscribeSessionStream,
  type AgUiEvent,
  type StreamStatus,
} from "../lib/api";
import type { ApiTarget } from "../lib/targets";

const store = new Map<string, string>();

const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
};

interface FetchCall {
  readonly url: string;
  readonly init?: RequestInit;
}

const calls: FetchCall[] = [];

type Handler = (url: string, init?: RequestInit) => Response | Promise<Response>;

const stubFetch = (handler: Handler): void => {
  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string" ? input : input instanceof Request ? input.url : input.toString();
    calls.push({ url, init });
    return handler(url, init);
  });
};

const jsonOk =
  (body: unknown): Handler =>
  () =>
    Response.json(body);
const status =
  (code: number): Handler =>
  () =>
    new Response("nope", { status: code });

beforeEach(() => {
  calls.length = 0;
  store.clear();
  vi.stubGlobal("localStorage", storage);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("GET wrappers", () => {
  it("listSessions unwraps the sessions array", async () => {
    stubFetch(jsonOk({ sessions: [{ id: "s1" }] }));
    await expect(listSessions()).resolves.toEqual([{ id: "s1" }]);
    expect(calls[0]?.url).toBe("/api/sessions");
  });

  it("getNode hits /api/node", async () => {
    stubFetch(jsonOk({ id: "node_1" }));
    await expect(getNode()).resolves.toEqual({ id: "node_1" });
    expect(calls[0]?.url).toBe("/api/node");
  });

  it("listAgents and getUserInfo unwrap their envelope", async () => {
    stubFetch((url) =>
      url === "/api/agents"
        ? Response.json({ agents: [{ id: "devin" }] })
        : Response.json({ user: { username: "me" } }),
    );
    await expect(listAgents()).resolves.toEqual([{ id: "devin" }]);
    await expect(getUserInfo()).resolves.toEqual({ username: "me" });
  });

  it("listDirs unwraps dirs and encodes the path", async () => {
    stubFetch(jsonOk({ dirs: ["/a"] }));
    await expect(listDirs("/tmp/some dir")).resolves.toEqual(["/a"]);
    expect(calls[0]?.url).toBe("/api/fs?path=%2Ftmp%2Fsome%20dir");
  });

  it("listProjects returns the envelope; getConfig returns config", async () => {
    stubFetch((url) =>
      url === "/api/projects"
        ? Response.json({ projects: [{ id: "p1" }] })
        : Response.json({ config: { k: 1 } }),
    );
    await expect(listProjects()).resolves.toEqual({ projects: [{ id: "p1" }] });
    await expect(getConfig()).resolves.toEqual({ config: { k: 1 } });
  });

  it("getHistory builds the query string from limit/before/agent", async () => {
    stubFetch(jsonOk({ messages: [], total: 0, start: 0 }));
    await getHistory("sess 1", { limit: 5, before: 9, agent: "devin" });
    expect(calls[0]?.url).toBe("/api/sessions/sess%201/history?limit=5&before=9&agent=devin");

    calls.length = 0;
    await getHistory("s1");
    expect(calls[0]?.url).toBe("/api/sessions/s1/history");
  });
});

describe("POST/PATCH/DELETE wrappers", () => {
  it("createSession posts the input as JSON", async () => {
    stubFetch(jsonOk({ id: "new-1", agentId: "devin" }));
    await expect(createSession({ cwd: "/work", title: "T" })).resolves.toEqual({
      id: "new-1",
      agentId: "devin",
    });
    expect(calls[0]?.init?.method).toBe("POST");
    expect(calls[0]?.init?.headers).toMatchObject({ "content-type": "application/json" });
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ cwd: "/work", title: "T" });
  });

  it("attach omits the body when no options are given", async () => {
    stubFetch(jsonOk({ attached: true, readOnly: true }));
    await attach("s1");
    expect(calls[0]?.url).toBe("/api/sessions/s1/attach");
    expect(calls[0]?.init?.body).toBeUndefined();
    expect(calls[0]?.init?.headers).not.toMatchObject({
      "content-type": "application/json",
    });
  });

  it("attach sends takeover/model/fallbacks and the ?agent= scope", async () => {
    stubFetch(jsonOk({ attached: true, readOnly: false }));
    await attach("s1", { takeover: true, model: "m", fallbacks: ["f1"], agent: "cline" });
    expect(calls[0]?.url).toBe("/api/sessions/s1/attach?agent=cline");
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({
      takeover: true,
      model: "m",
      fallbacks: ["f1"],
    });
  });

  it("an empty agent string does not append a query", async () => {
    stubFetch(jsonOk({ attached: true, readOnly: true }));
    await attach("s1", { agent: "" });
    expect(calls[0]?.url).toBe("/api/sessions/s1/attach");
  });

  it("patchSessionMeta returns res.ok without throwing on failure", async () => {
    stubFetch(status(500));
    await expect(patchSessionMeta("s1", { title: "x" })).resolves.toBe(false);
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ title: "x" });
  });

  it("renameSession resolves true on ok and throws the friendly error otherwise", async () => {
    stubFetch(status(200));
    await expect(renameSession("s1", "New", "devin")).resolves.toBe(true);
    expect(calls[0]?.url).toBe("/api/sessions/s1?agent=devin");

    stubFetch(status(404));
    await expect(renameSession("s1", "New")).rejects.toThrow("Not found");
  });

  it("deleteSession resolves true on ok and throws otherwise", async () => {
    stubFetch(status(200));
    await expect(deleteSession("s1")).resolves.toBe(true);
    expect(calls[0]?.init?.method).toBe("DELETE");

    stubFetch(status(403));
    await expect(deleteSession("s1")).rejects.toThrow("Access denied");
  });

  it("renameProject and deleteProject return res.ok", async () => {
    stubFetch(status(200));
    await expect(renameProject("p 1", "Name")).resolves.toBe(true);
    expect(calls[0]?.url).toBe("/api/projects/p%201");
    await expect(deleteProject("p1")).resolves.toBe(true);
    stubFetch(status(404));
    await expect(deleteProject("p1")).resolves.toBe(false);
  });

  it("createProject posts the name", async () => {
    stubFetch(jsonOk({ project: { id: "p1", name: "Alpha" } }));
    await expect(createProject("Alpha")).resolves.toEqual({ project: { id: "p1", name: "Alpha" } });
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ name: "Alpha" });
  });

  it("setConfigKey PATCHes /api/config/:key", async () => {
    stubFetch(jsonOk({ key: "k", value: false }));
    await setConfigKey("ui.flag", false);
    expect(calls[0]?.url).toBe("/api/config/ui.flag");
    expect(calls[0]?.init?.method).toBe("PATCH");
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ value: false });
  });

  it("convertSession posts the target agent under the fromAgent scope", async () => {
    stubFetch(jsonOk({ sessionId: "copy-1" }));
    await expect(convertSession("s1", "cline", "devin")).resolves.toEqual({
      sessionId: "copy-1",
    });
    expect(calls[0]?.url).toBe("/api/sessions/s1/convert?agent=devin");
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ agent: "cline" });
  });

  it("sendPrompt, cancel, and respondToPermission unwrap { ok }", async () => {
    stubFetch(jsonOk({ ok: true }));
    await expect(sendPrompt("s1", "go")).resolves.toBe(true);
    expect(calls[0]?.url).toBe("/api/sessions/s1/prompt");
    expect(JSON.parse(calls[0]?.init?.body as string)).toEqual({ text: "go" });

    await expect(cancel("s1", "cline")).resolves.toBe(true);
    expect(calls[1]?.url).toBe("/api/sessions/s1/cancel?agent=cline");
    expect(calls[1]?.init?.method).toBe("POST");

    await expect(respondToPermission("s1", "req-1", null)).resolves.toBe(true);
    expect(JSON.parse(calls[2]?.init?.body as string)).toEqual({
      requestId: "req-1",
      optionId: null,
    });
  });
});

describe("error mapping", () => {
  it.each([
    [400, "The server rejected the request"],
    [403, "Access denied"],
    [404, "Not found"],
    [409, "That operation is busy — try again in a moment"],
    [500, "The server hit an error — try again"],
    [503, "The server hit an error — try again"],
    [418, "Request failed (418)"],
  ])("maps HTTP %i to a friendly message", async (code, message) => {
    stubFetch(status(code));
    await expect(listSessions()).rejects.toThrow(message);
  });

  it("throws AuthError on 401", async () => {
    stubFetch(status(401));
    const failure = await listSessions().catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(AuthError);
    expect((failure as Error).name).toBe("AuthError");
  });

  it("reports unreachable servers", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("fetch failed");
    });
    await expect(listSessions()).rejects.toThrow("Can't reach the Sepia server");
  });
});

describe("ApiTarget", () => {
  it("prefixes baseUrl and sends the target's token", async () => {
    stubFetch(jsonOk({ sessions: [] }));
    const target: ApiTarget = { baseUrl: "https://peer.example", token: "peer-token" };
    await listSessions(target);
    expect(calls[0]?.url).toBe("https://peer.example/api/sessions");
    expect(calls[0]?.init?.headers).toMatchObject({ authorization: "Bearer peer-token" });
  });

  it("uses the stored local token by default", async () => {
    setToken("local-token");
    stubFetch(jsonOk({ sessions: [] }));
    await listSessions();
    expect(calls[0]?.init?.headers).toMatchObject({ authorization: "Bearer local-token" });
  });

  it("applies a timeout signal when the target sets one", async () => {
    stubFetch(jsonOk({ sessions: [] }));
    await listSessions({ baseUrl: "", token: null, timeoutMs: 5_000 });
    expect(calls[0]?.init?.signal).toBeInstanceOf(AbortSignal);
  });
});

describe("subscribeSessionStream", () => {
  class FakeEventSource {
    static instances: FakeEventSource[] = [];
    readonly url: string;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    onmessage: ((message: { data: string }) => void) | null = null;
    closed = false;

    constructor(url: string) {
      this.url = url;
      FakeEventSource.instances.push(this);
    }

    close(): void {
      this.closed = true;
    }
  }

  beforeEach(() => {
    FakeEventSource.instances = [];
    vi.stubGlobal("EventSource", FakeEventSource);
  });

  it("returns a no-op unsubscribe when EventSource is missing", () => {
    vi.stubGlobal("EventSource", undefined);
    const events: AgUiEvent[] = [];
    const unsubscribe = subscribeSessionStream("s1", (event) => events.push(event));
    expect(() => unsubscribe()).not.toThrow();
    expect(events).toEqual([]);
  });

  it("opens a stream at the session URL with token and agent params", () => {
    setToken("tok");
    subscribeSessionStream("s 1", () => {}, undefined, "cline");
    const source = FakeEventSource.instances[0];
    expect(source?.url).toBe("/api/sessions/s%201/stream?access_token=tok&agent=cline");
  });

  it("a peer target prefixes the URL and skips an unset token", () => {
    subscribeSessionStream("s1", () => {}, undefined, undefined, {
      baseUrl: "https://peer.example",
      token: null,
    });
    expect(FakeEventSource.instances[0]?.url).toBe("https://peer.example/api/sessions/s1/stream");
  });

  it("reports status transitions and forwards parsed events", () => {
    const statuses: StreamStatus[] = [];
    const events: AgUiEvent[] = [];
    subscribeSessionStream(
      "s1",
      (event) => events.push(event),
      (status) => statuses.push(status),
    );
    const source = FakeEventSource.instances[0];
    expect(statuses).toEqual(["connecting"]);

    source?.onopen?.();
    expect(statuses).toEqual(["connecting", "live"]);

    source?.onmessage?.({ data: JSON.stringify({ type: "RUN_FINISHED" }) });
    source?.onmessage?.({ data: "not json" }); // keep-alive / junk frame — ignored
    expect(events).toEqual([{ type: "RUN_FINISHED" }]);

    source?.onerror?.();
    expect(statuses).toEqual(["connecting", "live", "reconnecting"]);
  });

  it("the unsubscribe closes the source", () => {
    const unsubscribe = subscribeSessionStream("s1", () => {});
    unsubscribe();
    expect(FakeEventSource.instances[0]?.closed).toBe(true);
  });
});

describe("resumeSession", () => {
  const messages = (ids: ReadonlyArray<number>) =>
    ids.map((n) => ({ role: "user" as const, content: `m${n}`, createdAt: n * 1000 }));

  const RESUMED = {
    id: "imported-1",
    title: "Resumed",
    cwd: "/work",
    agent: "cline",
    updatedAt: new Date(0).toISOString(),
    locked: false,
    lockHolderPid: null,
    source: "cline",
    busy: false,
    pinned: false,
    archived: false,
    projectIds: [],
    model: null,
    spans: [],
  };

  it("pages backwards through history and posts the full IR to the target node", async () => {
    stubFetch((url, init) => {
      if (url.includes("/history")) {
        const before = new URL(url, "http://x").searchParams.get("before");
        return before === null
          ? Response.json({ messages: messages([3, 4]), total: 5, start: 3 })
          : Response.json({ messages: messages([0, 1, 2]), total: 5, start: 0 });
      }
      if (url.endsWith("/api/sessions/import") && init?.method === "POST") {
        return Response.json(RESUMED, { status: 201 });
      }
      return new Response("nope", { status: 404 });
    });

    const source: ApiTarget = { baseUrl: "https://src.example", token: "s-tok" };
    const target: ApiTarget = { baseUrl: "https://dst.example", token: "d-tok" };
    const result = await resumeSession(source, "s 1", "cline", target, {
      fromAgent: "devin",
      cwd: "/work",
      title: "Resumed",
    });

    expect(result).toEqual(RESUMED);

    // Two history pages: latest window first, then before=start until 0.
    expect(calls[0]?.url).toBe(
      "https://src.example/api/sessions/s%201/history?limit=500&agent=devin",
    );
    expect(calls[1]?.url).toBe(
      "https://src.example/api/sessions/s%201/history?limit=500&before=3&agent=devin",
    );

    // The import lands on the target node, with every message in order.
    const post = calls[2];
    expect(post?.url).toBe("https://dst.example/api/sessions/import");
    expect(post?.init?.method).toBe("POST");
    expect(post?.init?.headers).toMatchObject({ authorization: "Bearer d-tok" });
    const body = JSON.parse(post?.init?.body as string) as {
      agent: string;
      cwd: string;
      title: string;
      history: Array<{ content: string }>;
    };
    expect(body.agent).toBe("cline");
    expect(body.cwd).toBe("/work");
    expect(body.title).toBe("Resumed");
    expect(body.history.map((m) => m.content)).toEqual(["m0", "m1", "m2", "m3", "m4"]);
  });

  it("sends a single page when start is already 0", async () => {
    stubFetch((url, _init) =>
      url.includes("/history")
        ? Response.json({ messages: messages([0]), total: 1, start: 0 })
        : Response.json(RESUMED, { status: 201 }),
    );

    const result = await resumeSession(undefined, "s1", "devin", undefined);
    expect(result).toEqual(RESUMED);
    expect(calls[0]?.url).toBe("/api/sessions/s1/history?limit=500");
    expect(calls[1]?.url).toBe("/api/sessions/import");
    const body = JSON.parse(calls[1]?.init?.body as string) as { history: unknown[] };
    expect(body.history).toHaveLength(1);
  });
});

describe("pairNode", () => {
  it("posts the code to the node with no credentials and returns the token", async () => {
    stubFetch((url, init) => {
      expect(url).toBe("https://peer.example/api/pair");
      expect(init?.method).toBe("POST");
      expect(JSON.parse(init?.body as string)).toEqual({ code: "ABCD-EFGH" });
      const headers = new Headers(init?.headers);
      expect(headers.has("authorization")).toBe(false);
      return Response.json({ token: "sepia_issued" });
    });

    await expect(
      // Even if the caller's target carries a token, pairing must not send it.
      pairNode("ABCD-EFGH", { baseUrl: "https://peer.example", token: "stored-secret" }),
    ).resolves.toEqual({ token: "sepia_issued" });
  });

  it("surfaces a friendly error when the code is rejected", async () => {
    stubFetch(status(404));
    await expect(pairNode("BAD", { baseUrl: "", token: null })).rejects.toThrow(/expired|used/);
  });

  it("maps other failures through the friendly table", async () => {
    stubFetch(status(500));
    await expect(pairNode("BAD", { baseUrl: "", token: null })).rejects.toThrow(
      "The server hit an error",
    );
  });
});
