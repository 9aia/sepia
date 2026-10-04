import { expect, test } from "vite-plus/test";
import { Effect, Either, Layer, Option } from "effect";
import type {
  AcpCapabilities,
  AcpConnection,
  AcpSessionInfo,
  AcpSessionUpdate,
  PermissionRequest,
  PromptPart,
} from "sepia-acp";
import { MessageNode, Session, SessionRepository, ToolCall } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";
import { layer } from "../src/ControlPlane.js";
import { ControlPlane, ControlError } from "../src/types.js";
import type {
  AgentRuntime,
  ControlPlaneOptions,
  ControlPlaneService,
  SessionRewinder,
} from "../src/types.js";

const CWD = "/work/repo";

const node = (
  nodeId: number,
  role: MessageNode["role"],
  input: { readonly toolCallId?: string; readonly toolCalls?: ReadonlyArray<ToolCall> } = {},
): MessageNode =>
  new MessageNode({
    nodeId,
    parentNodeId: nodeId === 0 ? Option.none() : Option.some(nodeId - 1),
    role,
    content: `${role} ${nodeId}`,
    createdAt: 1700000000 + nodeId,
    metadata: null,
    ...(input.toolCallId === undefined ? {} : { toolCallId: Option.some(input.toolCallId) }),
    ...(input.toolCalls === undefined ? {} : { toolCalls: [...input.toolCalls] }),
  });

const session = (
  id: string,
  nodes: ReadonlyArray<MessageNode>,
  backendType = "windsurf",
): Session =>
  new Session({
    id,
    title: `Session ${id}`,
    workingDirectory: CWD,
    backendType,
    model: "test-model",
    createdAt: 0,
    lastActivityAt: 1700000000,
    mainChainId: nodes.length - 1,
    metadata: null,
    nodes,
  });

const chat = (id: string, backendType = "windsurf"): Session =>
  session(
    id,
    [
      node(0, "user"),
      node(1, "assistant"),
      node(2, "user"),
      node(3, "assistant", {
        toolCalls: [new ToolCall({ id: "call_1", name: "edit", arguments: {} })],
      }),
      node(4, "tool", { toolCallId: "call_1" }),
    ],
    backendType,
  );

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

const CAPABILITIES: AcpCapabilities = {
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

class FakeConnection implements AcpConnection {
  readonly capabilities = CAPABILITIES;
  infos: ReadonlyArray<AcpSessionInfo> = [];
  closed = false;
  private promptGate: Promise<void> | null = null;

  async listSessions(): Promise<ReadonlyArray<AcpSessionInfo>> {
    return this.infos;
  }
  async newSession(): Promise<string> {
    return "new";
  }
  async loadSession(): Promise<void> {}
  async prompt(_id: string, _parts: ReadonlyArray<PromptPart>): Promise<void> {
    if (this.promptGate !== null) await this.promptGate;
  }
  async cancel(): Promise<void> {}
  async deleteSession(): Promise<void> {}
  respondToPermission(): boolean {
    return true;
  }
  recentStderr(): ReadonlyArray<string> {
    return [];
  }
  onUpdate(_listener: (update: AcpSessionUpdate) => void): () => void {
    return () => {};
  }
  onPermission(_listener: (request: PermissionRequest) => void): () => void {
    return () => {};
  }
  /** Block `prompt` until the returned release runs — the session looks busy. */
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
  async close(): Promise<void> {
    this.closed = true;
  }
}

const fakeAgent = (conn: AcpConnection, id = "devin"): AgentRuntime => ({
  id,
  label: id,
  spawn: async () => conn,
});

const makeService = (
  options: ControlPlaneOptions,
  repo: SessionRepositoryService,
): Promise<ControlPlaneService> =>
  Effect.runPromise(
    Effect.gen(function* () {
      return yield* ControlPlane;
    }).pipe(Effect.provide(layer(options)), Effect.provide(Layer.succeed(SessionRepository, repo))),
  );

const runEither = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.either(effect));

/** Records the truncate call so tests can assert what the store writer saw. */
const fakeRewinder = (
  inner?: (session: Session, removed: ReadonlyArray<MessageNode>) => void | Error,
): {
  rewinder: SessionRewinder;
  calls: Array<{ session: Session; removed: number[]; truncated: Session }>;
} => {
  const calls: Array<{ session: Session; removed: number[]; truncated: Session }> = [];
  return {
    calls,
    rewinder: {
      truncate: (s, plan, truncated) =>
        Effect.try({
          try: () => {
            calls.push({ session: s, removed: plan.removed.map((n) => n.nodeId), truncated });
            const outcome = inner?.(s, plan.removed);
            if (outcome instanceof Error) throw outcome;
          },
          catch: (cause) => cause,
        }),
    },
  };
};

test("rewind requires confirm: true", async () => {
  const { rewinder } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: false, nodeId: 2 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left.code).toBe("invalid");
    expect(result.left.message).toContain("confirm");
  }
});

test("rewind needs exactly one selector", async () => {
  const { rewinder } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  for (const req of [
    { confirm: true },
    { confirm: true, nodeId: 2, turns: 1 },
    { confirm: true, nodeId: 42 },
  ] as const) {
    const result = await runEither(cp.rewind("s1", req));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
  }
});

test("rewind fails not_found for unknown sessions", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: {} },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("nope", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
});

test("rewind refuses while the session is busy", async () => {
  const conn = new FakeConnection();
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(conn)], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  await Effect.runPromise(cp.attach("s1"));
  const release = conn.holdPrompt();
  const pending = Effect.runPromise(Effect.either(cp.prompt("s1", [{ type: "text", text: "hi" }])));
  await new Promise((r) => setTimeout(r, 10));
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("busy");
  expect(calls).toEqual([]);
  release();
  await pending;
});

test("rewind refuses a locked session", async () => {
  const conn = new FakeConnection();
  conn.infos = [
    {
      sessionId: "s1",
      cwd: CWD,
      title: "s1",
      updatedAt: "",
      locked: true,
      lockHolderPid: 4321,
    },
  ];
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(conn)], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left.code).toBe("locked");
    expect(result.left.message).toContain("4321");
  }
  expect(calls).toEqual([]);
});

test("rewind on a backend with no rewinder fails conflict", async () => {
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: {} },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left.code).toBe("conflict");
    expect(result.left.message).toContain("not supported");
  }
});

test("rewind hands the store writer the prefix cut", async () => {
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isRight(result)).toBe(true);
  if (Either.isRight(result)) {
    expect(result.right).toEqual({ kept: 2, removed: 3 });
  }
  expect(calls.length).toBe(1);
  expect(calls[0]!.removed).toEqual([2, 3, 4]);
  // The truncated session's tail markers moved back to the kept tip.
  expect(calls[0]!.truncated.nodes.map((n) => n.nodeId)).toEqual([0, 1]);
  expect(calls[0]!.truncated.mainChainId).toBe(1);
});

test("rewind turns drops the last user turn", async () => {
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, turns: 1 }));
  expect(Either.isRight(result)).toBe(true);
  if (Either.isRight(result)) expect(result.right).toEqual({ kept: 2, removed: 3 });
  expect(calls[0]!.removed).toEqual([2, 3, 4]);
});

test("rewind at the tail is a no-op without calling the writer", async () => {
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 4 }));
  expect(Either.isRight(result)).toBe(true);
  if (Either.isRight(result)) expect(result.right).toEqual({ kept: 5, removed: 0 });
  expect(calls).toEqual([]);
});

test("rewind detaches a live session before truncating", async () => {
  const conn = new FakeConnection();
  const { rewinder } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(conn)], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const attached = await runEither(cp.attach("s1"));
  expect(Either.isRight(attached)).toBe(true);

  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isRight(result)).toBe(true);
  // The attach's agent connection was closed before the store write.
  expect(conn.closed).toBe(true);
});

test("rewind maps a ControlError from the writer through unchanged", async () => {
  const rewinder: SessionRewinder = {
    truncate: () =>
      Effect.fail(new ControlError({ code: "locked", message: "store held", cause: null })),
  };
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("locked");
});

test("rewind wraps a non-ControlError writer failure as internal", async () => {
  const rewinder: SessionRewinder = {
    truncate: () => Effect.fail(new Error("disk exploded")),
  };
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { devin: rewinder } },
    repository([chat("s1")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("internal");
});

test("rewind routes by the session's backend, not the request agent", async () => {
  const { rewinder, calls } = fakeRewinder();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], rewinders: { cursor: rewinder } },
    repository([chat("s1", "cursor")]),
  );
  const result = await runEither(cp.rewind("s1", { confirm: true, nodeId: 1 }));
  expect(Either.isRight(result)).toBe(true);
  expect(calls.length).toBe(1);
});
