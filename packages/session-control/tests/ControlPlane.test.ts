import { expect, test } from "vite-plus/test";
import { Effect, Either, Layer, Option } from "effect";
import { EventType } from "sepia-agui";
import type { Event } from "sepia-agui";
import type {
  AcpConnection,
  AcpSessionInfo,
  AcpSessionUpdate,
  PermissionRequest,
  PromptPart,
} from "sepia-acp";
import { MessageNode, Session, SessionRepository } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";
import { layer } from "../src/ControlPlane.js";
import { ControlPlane } from "../src/types.js";
import type { AgentRuntime, ControlPlaneOptions, ControlPlaneService } from "../src/types.js";

const session = (
  id: string,
  cwd: string,
  nodes: ReadonlyArray<MessageNode> = [],
  backendType = "windsurf",
): Session =>
  new Session({
    id,
    title: `Session ${id}`,
    workingDirectory: cwd,
    backendType,
    model: "test-model",
    createdAt: 0,
    lastActivityAt: 1_700_000_000,
    mainChainId: 0,
    metadata: null,
    nodes,
  });

const node = (
  nodeId: number,
  role: "system" | "user" | "assistant" | "tool",
  content: string,
  createdAt: number,
  toolName?: string,
): MessageNode =>
  new MessageNode({
    nodeId,
    role,
    content,
    createdAt,
    metadata: null,
    toolName: toolName === undefined ? Option.none() : Option.some(toolName),
  });

const repository = (sessions: ReadonlyArray<Session>): SessionRepositoryService => {
  const store = new Map(sessions.map((item) => [item.id, item]));
  return SessionRepository.of({
    save: (item) =>
      Effect.sync(() => {
        store.set(item.id, item);
      }),
    getById: (id) => Effect.sync(() => Option.fromNullable(store.get(id))),
    list: () => Effect.sync(() => [...store.values()]),
    delete: (id) =>
      Effect.sync(() => {
        store.delete(id);
      }),
    hasSession: (id) => Effect.sync(() => store.has(id)),
  });
};

class FakeConnection implements AcpConnection {
  readonly capabilities = { loadSession: true, sessionList: true };
  infos: ReadonlyArray<AcpSessionInfo> = [];
  listError: unknown = undefined;
  newSessionError: unknown = undefined;
  loadSessionError: unknown = undefined;
  permissionSettled = true;
  loaded: string[] = [];
  prompted: Array<{ id: string; parts: ReadonlyArray<PromptPart> }> = [];
  cancelled: string[] = [];
  permissions: Array<{ requestId: string; optionId: string | null }> = [];
  closed = false;
  private promptGate: Promise<void> | null = null;
  private updateListener: ((update: AcpSessionUpdate) => void) | null = null;
  private permissionListener: ((request: PermissionRequest) => void) | null = null;

  async listSessions(): Promise<ReadonlyArray<AcpSessionInfo>> {
    if (this.listError !== undefined) throw this.listError;
    return this.infos;
  }

  async newSession(_cwd: string): Promise<string> {
    if (this.newSessionError !== undefined) throw this.newSessionError;
    return "new";
  }

  async loadSession(sessionId: string, _cwd: string): Promise<void> {
    if (this.loadSessionError !== undefined) throw this.loadSessionError;
    this.loaded.push(sessionId);
  }

  async prompt(sessionId: string, parts: ReadonlyArray<PromptPart>): Promise<void> {
    this.prompted.push({ id: sessionId, parts });
    if (this.promptGate !== null) await this.promptGate;
  }

  async cancel(sessionId: string): Promise<void> {
    this.cancelled.push(sessionId);
  }

  deleted: string[] = [];

  async deleteSession(sessionId: string): Promise<void> {
    this.deleted.push(sessionId);
  }

  respondToPermission(requestId: string, optionId: string | null): boolean {
    this.permissions.push({ requestId, optionId });
    return this.permissionSettled;
  }

  recentStderr(): ReadonlyArray<string> {
    return [];
  }

  onUpdate(listener: (update: AcpSessionUpdate) => void): () => void {
    this.updateListener = listener;
    return () => {
      this.updateListener = null;
    };
  }

  onPermission(listener: (request: PermissionRequest) => void): () => void {
    this.permissionListener = listener;
    return () => {
      this.permissionListener = null;
    };
  }

  async close(): Promise<void> {
    this.closed = true;
  }

  /** Blocks `prompt` until the returned release function runs. */
  holdPrompt(): () => void {
    let release: () => void = () => {};
    this.promptGate = new Promise((resolve) => {
      release = resolve;
    });
    return () => {
      this.promptGate = null;
      release();
    };
  }

  pushUpdate(update: AcpSessionUpdate): void {
    this.updateListener?.(update);
  }

  pushPermission(request: PermissionRequest): void {
    this.permissionListener?.(request);
  }
}

const fakeAgent = (
  conn: AcpConnection,
  id = "devin",
): { readonly runtime: AgentRuntime; readonly spawns: Array<{ cwd: string }> } => {
  const spawns: Array<{ cwd: string }> = [];
  const runtime: AgentRuntime = {
    id,
    label: "Devin",
    spawn: async (options) => {
      spawns.push({ cwd: options.cwd });
      return conn;
    },
  };
  return { runtime, spawns };
};

const makeService = (
  options: ControlPlaneOptions,
  repo: SessionRepositoryService,
): Promise<ControlPlaneService> =>
  Effect.runPromise(
    Effect.gen(function* () {
      return yield* ControlPlane;
    }).pipe(Effect.provide(layer(options)), Effect.provide(Layer.succeed(SessionRepository, repo))),
  );

const runEither = <A, E>(effect: Effect.Effect<A, E>): Promise<Either.Either<A, E>> =>
  Effect.runPromise(Effect.either(effect));

const lockedInfo = (sessionId: string): AcpSessionInfo => ({
  sessionId,
  cwd: "/work",
  title: "One",
  updatedAt: "2024-01-01T00:00:00.000Z",
  locked: true,
  lockHolderPid: 42,
});

test("maps stored sessions to summaries", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([session("s1", "/work")]),
  );

  expect(await Effect.runPromise(cp.listSessions())).toEqual([
    {
      id: "s1",
      title: "Session s1",
      cwd: "/work",
      agent: "devin",
      updatedAt: new Date(1_700_000_000 * 1000).toISOString(),
      locked: false,
      lockHolderPid: null,
      source: "devin",
      busy: false,
    },
  ]);
});

test("maps a cline backend to the cline agent", async () => {
  const cp = await makeService(
    {
      agents: [
        fakeAgent(new FakeConnection(), "devin").runtime,
        fakeAgent(new FakeConnection(), "cline").runtime,
      ],
    },
    repository([session("s1", "/work", [], "cline")]),
  );

  const [summary] = await Effect.runPromise(cp.listSessions());
  expect(summary?.agent).toBe("cline");
  expect(summary?.source).toBe("cline");
});

test("merges agent lock state when withLocks is set", async () => {
  const conn = new FakeConnection();
  conn.infos = [lockedInfo("s1")];
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService(
    { agents: [runtime], probeCwd: "/work" },
    repository([session("s1", "/work"), session("s2", "/other")]),
  );

  const summaries = await Effect.runPromise(cp.listSessions({ withLocks: true }));

  expect(summaries.map((item) => [item.id, item.locked, item.lockHolderPid])).toEqual([
    ["s1", true, 42],
    ["s2", false, null],
  ]);
  expect(spawns).toEqual([{ cwd: "/work" }]);
  expect(conn.closed).toBe(true);
});

test("probes locks once for two rapid lists", async () => {
  const conn = new FakeConnection();
  conn.infos = [lockedInfo("s1")];
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService(
    { agents: [runtime], probeCwd: "/work" },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.listSessions({ withLocks: true }));
  await Effect.runPromise(cp.listSessions({ withLocks: true }));

  expect(spawns).toHaveLength(1);
});

test("probes locks under probeCwd, not a session directory", async () => {
  const conn = new FakeConnection();
  conn.infos = [lockedInfo("s1")];
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService(
    { agents: [runtime], probeCwd: "/probe" },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.listSessions({ withLocks: true }));

  expect(spawns).toEqual([{ cwd: "/probe" }]);
});

test("keeps listing when the lock pass fails", async () => {
  const conn = new FakeConnection();
  conn.listError = new Error("agent down");
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  const result = await runEither(cp.listSessions({ withLocks: true }));

  expect(Either.isRight(result)).toBe(true);
  if (Either.isRight(result)) expect(result.right.map((item) => item.locked)).toEqual([false]);
  expect(conn.closed).toBe(true);
});

test("returns history in node order with millisecond timestamps", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([
      session("s1", "/work", [
        node(1, "user", "hi", 10),
        node(2, "assistant", "hello", 11),
        node(3, "tool", "out", 12, "read_file"),
      ]),
    ]),
  );

  expect(await Effect.runPromise(cp.getHistory("s1"))).toEqual({
    messages: [
      { role: "user", content: "hi", createdAt: 10_000 },
      { role: "assistant", content: "hello", createdAt: 11_000 },
      { role: "tool", content: "out", createdAt: 12_000, toolName: "read_file" },
    ],
    total: 3,
    start: 0,
  });
});

test("returns the last limit messages and the full node count", async () => {
  const nodes = Array.from({ length: 10 }, (_, index) =>
    node(index + 1, "user", `m${index + 1}`, index + 1),
  );
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([session("s1", "/work", nodes)]),
  );

  const page = await Effect.runPromise(cp.getHistory("s1", { limit: 3 }));

  expect(page.total).toBe(10);
  expect(page.messages.map((message) => message.content)).toEqual(["m8", "m9", "m10"]);
});

test("clamps limit=0 to a single newest message (no zero-width windows)", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([session("s1", "/work", [node(1, "user", "a", 1), node(2, "assistant", "b", 2)])]),
  );

  const page = await Effect.runPromise(cp.getHistory("s1", { limit: 0 }));

  // An unclamped 0 slices [total, total) = empty with start=total — a paged
  // client would then walk identical empty windows forever.
  expect(page.total).toBe(2);
  expect(page.messages).toHaveLength(1);
  expect(page.messages[0]?.content).toBe("b");
  expect(page.start).toBe(1);
});

test("before outside the range clamps instead of producing a weird window", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([session("s1", "/work", [node(1, "user", "a", 1), node(2, "assistant", "b", 2)])]),
  );

  const page = await Effect.runPromise(cp.getHistory("s1", { limit: 2, before: 99 }));

  expect(page.total).toBe(2);
  expect(page.messages.map((m) => m.content)).toEqual(["a", "b"]);
  expect(page.start).toBe(0);
});

test("fails with ControlError when the session is unknown", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );

  const result = await runEither(cp.getHistory("missing"));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left._tag).toBe("ControlError");
});

test("returns an empty history page for a live session not yet in the store", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );

  // The agent only flushes a fresh session to the store after the first
  // prompt; until then history must be an empty page, not not_found.
  await Effect.runPromise(cp.createSession({ cwd: "/work" }));

  expect(await Effect.runPromise(cp.getHistory("new"))).toEqual({
    messages: [],
    total: 0,
    start: 0,
  });

  const missing = await runEither(cp.getHistory("ghost"));
  expect(Either.isLeft(missing)).toBe(true);
  if (Either.isLeft(missing)) expect(missing.left.code).toBe("not_found");
});

test("attaches by spawning the agent and loading the session", async () => {
  const conn = new FakeConnection();
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService({ agents: [runtime] }, repository([session("s1", "/work")]));

  expect(await Effect.runPromise(cp.attach("s1"))).toEqual({
    attached: true,
    readOnly: false,
    agentId: "devin",
  });
  expect(spawns).toEqual([{ cwd: "/work" }]);
  expect(conn.loaded).toEqual(["s1"]);
});

test("returns read-only and skips loading when the session is locked", async () => {
  const conn = new FakeConnection();
  conn.infos = [lockedInfo("s1")];
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  expect(await Effect.runPromise(cp.attach("s1"))).toEqual({
    attached: false,
    readOnly: true,
    agentId: "devin",
  });
  expect(conn.loaded).toEqual([]);
  expect(conn.closed).toBe(true);
});

test("loads a locked session when takeover is requested", async () => {
  const conn = new FakeConnection();
  conn.infos = [lockedInfo("s1")];
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  expect(await Effect.runPromise(cp.attach("s1", { takeover: true }))).toEqual({
    attached: true,
    readOnly: false,
    agentId: "devin",
  });
  expect(conn.loaded).toEqual(["s1"]);
});

test("re-attaching a live session does not spawn again", async () => {
  const conn = new FakeConnection();
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService({ agents: [runtime] }, repository([session("s1", "/work")]));

  await Effect.runPromise(cp.attach("s1"));
  expect(await Effect.runPromise(cp.attach("s1"))).toEqual({
    attached: true,
    readOnly: false,
    agentId: "devin",
  });
  expect(spawns).toHaveLength(1);
  expect(conn.loaded).toEqual(["s1"]);
});

test("attach prefers the session's own agent", async () => {
  const devin = fakeAgent(new FakeConnection(), "devin");
  const cline = fakeAgent(new FakeConnection(), "cline");
  const cp = await makeService(
    { agents: [devin.runtime, cline.runtime], defaultAgentId: "devin" },
    repository([session("s1", "/work", [], "cline")]),
  );

  await Effect.runPromise(cp.attach("s1"));

  expect(cline.spawns).toHaveLength(1);
  expect(devin.spawns).toHaveLength(0);
});

test("treats a loadSession failure as authoritative and re-probes", async () => {
  const first = new FakeConnection();
  first.loadSessionError = new Error("locked elsewhere");
  const second = new FakeConnection();
  second.infos = [lockedInfo("s1")];
  let spawned = 0;
  const runtime: AgentRuntime = {
    id: "devin",
    label: "Devin",
    spawn: async () => (spawned++ === 0 ? first : second),
  };
  const cp = await makeService(
    { agents: [runtime], probeCwd: "/work" },
    repository([session("s1", "/work")]),
  );

  expect(await Effect.runPromise(cp.attach("s1"))).toEqual({
    attached: false,
    readOnly: true,
    agentId: "devin",
  });
  expect(first.closed).toBe(true);
  expect(spawned).toBe(2);
});

test("propagates a loadSession failure when the session is not locked", async () => {
  const conn = new FakeConnection();
  conn.loadSessionError = new Error("load failed");
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime], probeCwd: "/work" },
    repository([session("s1", "/work")]),
  );

  const result = await runEither(cp.attach("s1"));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.message).toContain("Failed to load session");
});

test("createSession spawns the agent, returns the id, and registers a live session", async () => {
  const conn = new FakeConnection();
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService({ agents: [runtime] }, repository([]));

  const result = await Effect.runPromise(cp.createSession({ cwd: "/work", title: "Fresh" }));

  expect(result).toEqual({ id: "new", agentId: "devin" });
  expect(spawns).toEqual([{ cwd: "/work" }]);

  await Effect.runPromise(cp.prompt("new", "hi"));
  expect(conn.prompted).toEqual([{ id: "new", parts: [{ type: "text", text: "hi" }] }]);
});

test("attach tolerates a live session that is not in the store", async () => {
  const conn = new FakeConnection();
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService({ agents: [runtime] }, repository([]));

  await Effect.runPromise(cp.createSession({ cwd: "/work" }));

  expect(await Effect.runPromise(cp.attach("new"))).toEqual({
    attached: true,
    readOnly: false,
    agentId: "devin",
  });
  expect(spawns).toHaveLength(1);
});

test("createSession fails for an unknown agent id", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );

  const result = await runEither(cp.createSession({ cwd: "/work", agentId: "nope" }));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left._tag).toBe("ControlError");
});

test("createSession rejects empty and relative cwd", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );

  expect(Either.isLeft(await runEither(cp.createSession({ cwd: "" })))).toBe(true);
  expect(Either.isLeft(await runEither(cp.createSession({ cwd: "relative/path" })))).toBe(true);
});

test("listSessions includes created sessions that are not yet in the store", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.createSession({ cwd: "/tmp/fresh", title: "Fresh" }));
  const summaries = await Effect.runPromise(cp.listSessions());

  expect(summaries.map((item) => item.id)).toEqual(["s1", "new"]);
  expect(summaries.find((item) => item.id === "new")).toMatchObject({
    title: "Fresh",
    cwd: "/tmp/fresh",
    agent: "devin",
    locked: false,
    lockHolderPid: null,
    source: "sepia",
  });
});

test("listSessions keeps synthesized live sessions when the lock pass fails", async () => {
  const conn = new FakeConnection();
  conn.listError = new Error("agent down");
  const cp = await makeService({ agents: [fakeAgent(conn).runtime] }, repository([]));

  await Effect.runPromise(cp.createSession({ cwd: "/work" }));
  const summaries = await Effect.runPromise(cp.listSessions({ withLocks: true }));

  expect(summaries.map((item) => item.id)).toEqual(["new"]);
});

test("prompt emits RUN_STARTED before RUN_FINISHED", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  const events: Event[] = [];
  await Effect.runPromise(
    cp.subscribe("s1", (batch) => {
      events.push(...batch);
    }),
  );
  await Effect.runPromise(cp.prompt("s1", "hi"));

  expect(events.map((event) => event.type)).toEqual([
    EventType.RUN_STARTED,
    EventType.RUN_FINISHED,
  ]);
  expect(conn.prompted).toEqual([{ id: "s1", parts: [{ type: "text", text: "hi" }] }]);
});

test("forwards agent updates to subscribers", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  const events: Event[] = [];
  await Effect.runPromise(
    cp.subscribe("s1", (batch) => {
      events.push(...batch);
    }),
  );
  conn.pushUpdate({ kind: "agent_message_chunk", text: "hello" });

  expect(events.map((event) => event.type)).toEqual([
    EventType.TEXT_MESSAGE_START,
    EventType.TEXT_MESSAGE_CONTENT,
  ]);
});

test("detach closes the connection and stops routing", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  const events: Event[] = [];
  const unsubscribe = await Effect.runPromise(
    cp.subscribe("s1", (batch) => {
      events.push(...batch);
    }),
  );
  unsubscribe();
  await Effect.runPromise(cp.prompt("s1", "hi"));
  expect(events).toEqual([]);

  await Effect.runPromise(cp.detach("s1"));
  expect(conn.closed).toBe(true);

  expect(Either.isLeft(await runEither(cp.prompt("s1", "again")))).toBe(true);
});

test("prompt before attach fails with ControlError", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([session("s1", "/work")]),
  );

  const result = await runEither(cp.prompt("s1", "hi"));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left._tag).toBe("ControlError");
});

test("rejects a concurrent prompt with busy and clears the flag after the turn", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );
  await Effect.runPromise(cp.attach("s1"));

  const release = conn.holdPrompt();
  const first = Effect.runPromise(cp.prompt("s1", "one"));
  await new Promise((resolve) => setTimeout(resolve, 0));

  const busy = await runEither(cp.prompt("s1", "two"));
  expect(Either.isLeft(busy)).toBe(true);
  if (Either.isLeft(busy)) expect(busy.left.code).toBe("busy");

  release();
  await first;

  await Effect.runPromise(cp.prompt("s1", "three"));
  expect(conn.prompted.map((item) => item.parts[0]?.text)).toEqual(["one", "three"]);
});

test("reports busy on the live summary while a turn is running", async () => {
  const conn = new FakeConnection();
  const cp = await makeService({ agents: [fakeAgent(conn).runtime] }, repository([]));
  await Effect.runPromise(cp.createSession({ cwd: "/work" }));

  const release = conn.holdPrompt();
  const first = Effect.runPromise(cp.prompt("new", "one"));
  await new Promise((resolve) => setTimeout(resolve, 0));

  const [summary] = await Effect.runPromise(cp.listSessions());
  expect(summary?.busy).toBe(true);

  release();
  await first;
});

test("lists configured agents", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );
  expect(cp.listAgents()).toEqual([{ id: "devin", label: "Devin" }]);
});

test("closeAll detaches every live session", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work"), session("s2", "/other")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  await Effect.runPromise(cp.attach("s2"));
  await Effect.runPromise(cp.closeAll());

  expect(conn.closed).toBe(true);
  expect(Either.isLeft(await runEither(cp.prompt("s1", "hi")))).toBe(true);
});

test("concurrent attaches spawn the agent exactly once", async () => {
  const conn = new FakeConnection();
  const spawns: Array<{ cwd: string }> = [];
  const runtime: AgentRuntime = {
    id: "devin",
    label: "Devin",
    spawn: async (options) => {
      spawns.push({ cwd: options.cwd });
      await new Promise((resolve) => setTimeout(resolve, 5));
      return conn;
    },
  };
  const cp = await makeService({ agents: [runtime] }, repository([session("s1", "/work")]));

  const [first, second] = await Promise.all([
    Effect.runPromise(cp.attach("s1")),
    Effect.runPromise(cp.attach("s1")),
  ]);

  expect(first).toEqual({ attached: true, readOnly: false, agentId: "devin" });
  expect(second).toEqual({ attached: true, readOnly: false, agentId: "devin" });
  expect(spawns).toHaveLength(1);
  expect(conn.loaded).toEqual(["s1"]);
});

test("createSession closes the connection when newSession rejects", async () => {
  const conn = new FakeConnection();
  conn.newSessionError = new Error("agent refused");
  const cp = await makeService({ agents: [fakeAgent(conn).runtime] }, repository([]));

  const result = await runEither(cp.createSession({ cwd: "/work" }));

  expect(Either.isLeft(result)).toBe(true);
  expect(conn.closed).toBe(true);
});

test("respondToPermission succeeds when the broker settles the request", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  await Effect.runPromise(cp.respondToPermission("s1", "req-1", "allow"));

  expect(conn.permissions).toEqual([{ requestId: "req-1", optionId: "allow" }]);
});

test("respondToPermission fails on an unknown request id", async () => {
  const conn = new FakeConnection();
  conn.permissionSettled = false;
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  const result = await runEither(cp.respondToPermission("s1", "nope", "allow"));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left._tag).toBe("ControlError");
    expect(result.left.message).toContain("Unknown permission request: nope");
  }
});

test("a throwing listener does not starve other subscribers", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime] },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  const logged: unknown[] = [];
  const original = console.error;
  console.error = (...args: unknown[]) => {
    logged.push(args);
  };
  try {
    await Effect.runPromise(
      cp.subscribe("s1", () => {
        throw new Error("listener boom");
      }),
    );
    const events: Event[] = [];
    await Effect.runPromise(
      cp.subscribe("s1", (batch) => {
        events.push(...batch);
      }),
    );
    conn.pushUpdate({ kind: "agent_message_chunk", text: "hi" });

    expect(events.map((event) => event.type)).toEqual([
      EventType.TEXT_MESSAGE_START,
      EventType.TEXT_MESSAGE_CONTENT,
    ]);
    expect(logged.length).toBeGreaterThan(0);
  } finally {
    console.error = original;
  }
});

test("the idle sweep detaches a session with no listeners after the TTL", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime], idleTtlMs: 50, sweepMs: 10 },
    repository([session("s1", "/work")]),
  );

  await Effect.runPromise(cp.attach("s1"));
  await new Promise((resolve) => setTimeout(resolve, 200));

  expect(conn.closed).toBe(true);
  expect(Either.isLeft(await runEither(cp.prompt("s1", "hi")))).toBe(true);
  await Effect.runPromise(cp.closeAll());
});

test("the idle sweep skips a session with an in-flight turn", async () => {
  const conn = new FakeConnection();
  const cp = await makeService(
    { agents: [fakeAgent(conn).runtime], idleTtlMs: 50, sweepMs: 10 },
    repository([session("s1", "/work")]),
  );
  await Effect.runPromise(cp.attach("s1"));

  const release = conn.holdPrompt();
  const first = Effect.runPromise(cp.prompt("s1", "one"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 200));
  expect(conn.closed).toBe(false);

  release();
  await first;
  await new Promise((resolve) => setTimeout(resolve, 200));
  expect(conn.closed).toBe(true);
  await Effect.runPromise(cp.closeAll());
});

test("deleteSession detaches a live session, then deletes through the agent", async () => {
  const conn = new FakeConnection();
  const { runtime, spawns } = fakeAgent(conn);
  const cp = await makeService({ agents: [runtime] }, repository([session("s1", "/work")]));

  await Effect.runPromise(cp.attach("s1"));
  await Effect.runPromise(cp.deleteSession("s1"));

  expect(conn.deleted).toEqual(["s1"]);
  expect(spawns).toHaveLength(2);
  expect(Either.isLeft(await runEither(cp.prompt("s1", "hi")))).toBe(true);
});

test("deleteSession fails for an unknown session", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection()).runtime] },
    repository([]),
  );

  const result = await runEither(cp.deleteSession("nope"));

  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
});
