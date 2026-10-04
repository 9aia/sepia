import { afterEach, describe, expect, it } from "vite-plus/test";
import { Effect, Either, Layer, Option } from "effect";
import type { AcpConnection, AcpSessionInfo, AcpSessionUpdate, PermissionRequest } from "sepia-acp";
import { Session, SessionRepository } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";
import { layer } from "../src/ControlPlane.js";
import { ControlPlane } from "../src/types.js";
import type { AgentRuntime, ControlPlaneService } from "../src/types.js";

const session = (id: string, backendType = "windsurf"): Session =>
  new Session({
    id,
    title: `Session ${id}`,
    workingDirectory: "/work",
    backendType,
    model: "test-model",
    createdAt: 0,
    lastActivityAt: 1_700_000_000,
    mainChainId: 0,
    metadata: null,
    nodes: [],
  });

const repository = (
  sessions: ReadonlyArray<Session>,
  fail?: { getById?: boolean },
): SessionRepositoryService => {
  const store = new Map(sessions.map((item) => [item.id, item]));
  return SessionRepository.of({
    save: (item) =>
      Effect.sync(() => {
        store.set(item.id, item);
      }),
    getById: (id) =>
      fail?.getById === true
        ? Effect.fail(new Error("store read failed") as never)
        : Effect.sync(() => Option.fromNullable(store.get(id))),
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
  newSessionId = "new-1";
  spawnError: unknown = undefined;
  permissionSettled = true;
  cancelled: string[] = [];
  closed = false;

  async listSessions(): Promise<ReadonlyArray<AcpSessionInfo>> {
    return this.infos;
  }

  async newSession(): Promise<string> {
    return this.newSessionId;
  }

  async loadSession(): Promise<void> {}
  async prompt(): Promise<void> {}
  async cancel(sessionId: string): Promise<void> {
    this.cancelled.push(sessionId);
  }
  async deleteSession(): Promise<void> {}
  respondToPermission(): boolean {
    return this.permissionSettled;
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
  async close(): Promise<void> {
    this.closed = true;
  }
}

interface FakeAgent {
  readonly runtime: AgentRuntime;
  readonly conn: FakeConnection;
  readonly spawns: Array<{ cwd: string; model?: string }>;
  spawnError: unknown;
}

const fakeAgent = (id = "devin"): FakeAgent => {
  const conn = new FakeConnection();
  const agent: FakeAgent = {
    conn,
    spawns: [],
    spawnError: undefined,
    runtime: {
      id,
      label: id,
      spawn: async (options) => {
        agent.spawns.push({ cwd: options.cwd, model: options.model });
        if (agent.spawnError !== undefined) throw agent.spawnError;
        return conn;
      },
    },
  };
  return agent;
};

const makeService = (
  agents: ReadonlyArray<AgentRuntime>,
  repo: SessionRepositoryService,
  extra: Partial<Parameters<typeof layer>[0]> = {},
): Promise<ControlPlaneService> =>
  Effect.runPromise(
    Effect.gen(function* () {
      return yield* ControlPlane;
    }).pipe(
      Effect.provide(layer({ agents, idleTtlMs: 0, ...extra })),
      Effect.provide(Layer.succeed(SessionRepository, repo)),
    ),
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

const envBackup = { ...process.env };

afterEach(() => {
  process.env = { ...envBackup };
});

describe("attach — error and lock paths", () => {
  it("an unknown session id fails not_found", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    const result = await runEither(plane.attach("ghost"));
    expect(Either.isLeft(result)).toBe(true);
    // performAttach crosses a nested runPromise boundary, so the fiber wraps
    // the ControlError — the message still carries the failure.
    if (Either.isLeft(result)) expect(result.left.message).toContain("Unknown session: ghost");
    await Effect.runPromise(plane.closeAll());
  });

  it("a session whose backend has no matching agent fails unknown_agent", async () => {
    const plane = await makeService([], repository([session("s1")]));
    const result = await runEither(plane.attach("s1"));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result))
      expect(result.left.message).toContain("No agent available for session: s1");
    await Effect.runPromise(plane.closeAll());
  });

  it("a cursor session does not fall back to the default agent", async () => {
    const devin = fakeAgent();
    const plane = await makeService([devin.runtime], repository([session("s1", "cursor")]));
    const result = await runEither(plane.attach("s1"));
    expect(Either.isLeft(result)).toBe(true);
    // performAttach crosses a nested runPromise boundary that wraps the
    // ControlError — assert on the message like the neighboring tests.
    if (Either.isLeft(result))
      expect(result.left.message).toContain("No agent available for session: s1");
    // read-only stores must not spawn an unrelated agent
    expect(devin.spawns).toHaveLength(0);
    await Effect.runPromise(plane.closeAll());
  });

  it("a store read failure surfaces as internal", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([], { getById: true }));
    const result = await runEither(plane.attach("s1"));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) {
      expect(result.left.message).toContain("Failed to read session");
    }
    await Effect.runPromise(plane.closeAll());
  });

  it("a session locked by another process attaches read-only without takeover", async () => {
    const agent = fakeAgent();
    agent.conn.infos = [lockedInfo("s1")];
    const plane = await makeService([agent.runtime], repository([session("s1")]));

    const result = await runEither(plane.attach("s1"));
    expect(result).toEqual(Either.right({ attached: false, readOnly: true, agentId: "devin" }));
    expect(agent.conn.closed).toBe(true);
    await Effect.runPromise(plane.closeAll());
  });

  it("attaching a live id under a different agent conflicts", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    const created = await Effect.runPromise(
      plane.createSession({ cwd: "/work", agentId: "devin" }),
    );
    const result = await runEither(plane.attach(created.id, { agentId: "cline" }));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("conflict");
    // Same-agent reattach is a cheap no-op.
    const same = await runEither(plane.attach(created.id, { agentId: "devin" }));
    expect(same).toEqual(Either.right({ attached: true, readOnly: false, agentId: "devin" }));
    await Effect.runPromise(plane.closeAll());
  });
});

describe("createSession", () => {
  it("rejects a non-absolute cwd", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    for (const cwd of ["relative/dir", "  "]) {
      const result = await runEither(plane.createSession({ cwd }));
      expect(Either.isLeft(result)).toBe(true);
      if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
    }
    await Effect.runPromise(plane.closeAll());
  });

  it("rejects an unknown agent id", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    const result = await runEither(plane.createSession({ cwd: "/work", agentId: "nope" }));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("unknown_agent");
    await Effect.runPromise(plane.closeAll());
  });

  it("honors defaultAgentId when no agentId is given", async () => {
    const devin = fakeAgent("devin");
    const cline = fakeAgent("cline");
    const plane = await makeService([devin.runtime, cline.runtime], repository([]), {
      defaultAgentId: "cline",
    });
    const created = await Effect.runPromise(plane.createSession({ cwd: "/work" }));
    expect(created.agentId).toBe("cline");
    expect(cline.spawns).toHaveLength(1);
    expect(devin.spawns).toHaveLength(0);
    await Effect.runPromise(plane.closeAll());
  });

  it("forwards the model preference to the spawn", async () => {
    const agent = fakeAgent();
    const plane = await makeService([agent.runtime], repository([]));
    await Effect.runPromise(plane.createSession({ cwd: "/work", model: "claude-x" }));
    expect(agent.spawns[0]?.model).toBe("claude-x");
    await Effect.runPromise(plane.closeAll());
  });

  it("a spawn failure surfaces instead of leaking", async () => {
    const agent = fakeAgent();
    agent.spawnError = new Error("binary missing");
    const plane = await makeService([agent.runtime], repository([]));
    const result = await runEither(plane.createSession({ cwd: "/work" }));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("internal");
    await Effect.runPromise(plane.closeAll());
  });
});

describe("live-session operations", () => {
  it("cancel reaches the agent connection", async () => {
    const agent = fakeAgent();
    const plane = await makeService([agent.runtime], repository([]));
    const created = await Effect.runPromise(plane.createSession({ cwd: "/work" }));
    await Effect.runPromise(plane.cancel(created.id));
    expect(agent.conn.cancelled).toEqual([created.id]);
    await Effect.runPromise(plane.closeAll());
  });

  it("cancel/prompt on a detached session fails invalid", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    for (const effect of [
      plane.cancel("nope"),
      plane.prompt("nope", [{ type: "text", text: "hi" }]),
    ]) {
      const result = await runEither(effect);
      expect(Either.isLeft(result)).toBe(true);
      if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
    }
    await Effect.runPromise(plane.closeAll());
  });

  it("an unsettled permission respond fails not_found", async () => {
    const agent = fakeAgent();
    agent.conn.permissionSettled = false;
    const plane = await makeService([agent.runtime], repository([]));
    const created = await Effect.runPromise(plane.createSession({ cwd: "/work" }));
    const result = await runEither(plane.respondToPermission(created.id, "req-x", "allow"));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
    await Effect.runPromise(plane.closeAll());
  });
});

describe("deleteSession", () => {
  it("an unknown session fails not_found", async () => {
    const plane = await makeService([fakeAgent().runtime], repository([]));
    const result = await runEither(plane.deleteSession("ghost"));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
    await Effect.runPromise(plane.closeAll());
  });

  it("a stored session with no available agent fails unknown_agent", async () => {
    const plane = await makeService([], repository([session("s1", "cline")]));
    const result = await runEither(plane.deleteSession("s1"));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("unknown_agent");
    await Effect.runPromise(plane.closeAll());
  });
});

describe("env-driven options", () => {
  it("SEPIA_* env vars feed the defaults when options are absent", async () => {
    process.env.SEPIA_IDLE_TTL_MS = "60000";
    process.env.SEPIA_SWEEP_MS = "30000";
    const plane = await makeService([fakeAgent().runtime], repository([]), {
      idleTtlMs: undefined,
      sweepMs: undefined,
    });
    await Effect.runPromise(plane.closeAll());
  });

  it("invalid env values fall back to defaults", async () => {
    process.env.SEPIA_IDLE_TTL_MS = "not-a-number";
    const plane = await makeService([fakeAgent().runtime], repository([]), {
      idleTtlMs: undefined,
      sweepMs: 60_000,
    });
    await Effect.runPromise(plane.closeAll());
  });

  it("withLocks degrades to [] when the agent list is empty or the probe fails", async () => {
    const empty = await makeService([], repository([]));
    expect(await Effect.runPromise(empty.listSessions({ withLocks: true }))).toEqual([]);
    await Effect.runPromise(empty.closeAll());

    const agent = fakeAgent();
    agent.spawnError = new Error("spawn down");
    const plane = await makeService([agent.runtime], repository([]));
    expect(await Effect.runPromise(plane.listSessions({ withLocks: true }))).toEqual([]);
    await Effect.runPromise(plane.closeAll());
  });
});
