import { expect, test } from "vite-plus/test";
import { Effect, Either, Layer, Option } from "effect";
import type {
  AcpConnection,
  AcpSessionInfo,
  AcpSessionUpdate,
  PermissionRequest,
  PromptPart,
} from "sepia-acp";
import { MessageNode, Session, SessionRepository, ToolCall } from "sepia-core";
import type { SessionRepositoryService } from "sepia-core";
import { layer } from "../src/ControlPlane.js";
import { ControlPlane } from "../src/types.js";
import type {
  AgentRuntime,
  ControlPlaneOptions,
  ControlPlaneService,
  RestoreExec,
} from "../src/types.js";

const CWD = "/work/repo";

const session = (
  id: string,
  nodes: ReadonlyArray<MessageNode> = [],
  checkpoints: Session["checkpoints"] = [],
  cwd = CWD,
): Session =>
  new Session({
    id,
    title: `Session ${id}`,
    workingDirectory: cwd,
    backendType: "windsurf",
    model: "test-model",
    createdAt: 0,
    lastActivityAt: 1_700_000_000,
    mainChainId: 0,
    checkpoints,
    metadata: null,
    nodes,
  });

const editNode = (
  nodeId: number,
  callId: string,
  diffs: ReadonlyArray<{ path: string; oldText?: string; newText?: string }>,
): MessageNode =>
  new MessageNode({
    nodeId,
    role: "assistant",
    content: "",
    createdAt: nodeId,
    metadata: null,
    toolCalls: [new ToolCall({ id: callId, name: "edit", arguments: {}, diffs: [...diffs] })],
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
  async close(): Promise<void> {}
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
}

const fakeAgent = (conn: AcpConnection, id = "devin"): AgentRuntime => ({
  id,
  label: id,
  spawn: async () => conn,
});

const enc = new TextEncoder();

/** In-memory RestoreExec: a file map plus a scripted git answer table. */
const fakeExec = (
  files: Record<string, string> = {},
  git?: (
    cwd: string,
    args: ReadonlyArray<string>,
  ) => { code: number; stdout?: string; stderr?: string },
) => {
  const fs = new Map<string, Uint8Array>(Object.entries(files).map(([k, v]) => [k, enc.encode(v)]));
  const exec: RestoreExec = {
    readFile: async (path) => fs.get(path) ?? null,
    writeFile: async (path, content) => {
      fs.set(path, content);
    },
    removeFile: async (path) => {
      fs.delete(path);
    },
    git: async (cwd, args) => {
      const out = git?.(cwd, args) ?? { code: 128, stderr: `unstubbed git ${args.join(" ")}` };
      return {
        code: out.code,
        stdout: enc.encode(out.stdout ?? ""),
        stderr: out.stderr ?? "",
      };
    },
  };
  return {
    exec,
    fs,
    text: (p: string) => (fs.get(p) === undefined ? null : new TextDecoder().decode(fs.get(p))),
  };
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

const runEither = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.either(effect));

const restoreSession = session("s1", [
  editNode(1, "c1", [{ path: "a.ts", oldText: "v0", newText: "v1" }]),
  editNode(2, "c2", [{ path: "a.ts", oldText: "v1", newText: "v2" }]),
  editNode(3, "c3", [{ path: "made.ts", newText: "created" }]),
]);

test("restore requires confirm: true", async () => {
  const { exec } = fakeExec();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await runEither(cp.restore("s1", { confirm: false, path: "a.ts" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left.code).toBe("invalid");
    expect(result.left.message).toContain("confirm");
  }
});

test("restore needs exactly one of path/checkpoint", async () => {
  const { exec } = fakeExec();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  for (const req of [{ confirm: true }, { confirm: true, path: "a.ts", checkpoint: "abc" }]) {
    const result = await runEither(cp.restore("s1", req));
    expect(Either.isLeft(result)).toBe(true);
    if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
  }
});

test("restore fails not_found for unknown sessions", async () => {
  const { exec } = fakeExec();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await runEither(cp.restore("nope", { confirm: true, path: "a.ts" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
});

test("restore refuses a locked session", async () => {
  const conn = new FakeConnection();
  conn.infos = [
    {
      sessionId: "s1",
      cwd: CWD,
      title: "s1",
      updatedAt: "2024-01-01T00:00:00.000Z",
      locked: true,
      lockHolderPid: 4242,
    },
  ];
  const { exec } = fakeExec({ [`${CWD}/a.ts`]: "v2" });
  const cp = await makeService(
    { agents: [fakeAgent(conn)], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await runEither(cp.restore("s1", { confirm: true, path: "a.ts" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) {
    expect(result.left.code).toBe("locked");
    expect(result.left.message).toContain("4242");
  }
});

test("restore refuses while the session is busy", async () => {
  const conn = new FakeConnection();
  const { exec } = fakeExec({ [`${CWD}/a.ts`]: "v2" });
  const cp = await makeService(
    { agents: [fakeAgent(conn)], restoreExec: exec },
    repository([restoreSession]),
  );
  await Effect.runPromise(cp.attach("s1"));
  const release = conn.holdPrompt();
  const pending = Effect.runPromise(Effect.either(cp.prompt("s1", [{ type: "text", text: "hi" }])));
  await new Promise((r) => setTimeout(r, 10));
  const result = await runEither(cp.restore("s1", { confirm: true, path: "a.ts" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("busy");
  release();
  await pending;
});

test("path restore rewrites the file to its pre-session state", async () => {
  const { exec, text } = fakeExec({ [`${CWD}/a.ts`]: "v2" });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await Effect.runPromise(cp.restore("s1", { confirm: true, path: "a.ts" }));
  expect(result.restored).toEqual([{ path: `${CWD}/a.ts`, action: "written", bytes: 2 }]);
  expect(text(`${CWD}/a.ts`)).toBe("v0");
});

test("path restore deletes a file the session created", async () => {
  const { exec, text } = fakeExec({ [`${CWD}/made.ts`]: "created" });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await Effect.runPromise(cp.restore("s1", { confirm: true, path: "made.ts" }));
  expect(result.restored).toEqual([{ path: `${CWD}/made.ts`, action: "deleted" }]);
  expect(text(`${CWD}/made.ts`)).toBeNull();
});

test("path restore skips a drifted file instead of clobbering it", async () => {
  const { exec, text } = fakeExec({ [`${CWD}/a.ts`]: "hand edited" });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await Effect.runPromise(cp.restore("s1", { confirm: true, path: "a.ts" }));
  expect(result.restored).toEqual([]);
  expect(result.skipped).toEqual([
    { path: "a.ts", reason: "file no longer contains the recorded after-state" },
  ]);
  expect(text(`${CWD}/a.ts`)).toBe("hand edited");
});

test("path restore rejects escapes outside the working directory", async () => {
  const { exec } = fakeExec();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await runEither(cp.restore("s1", { confirm: true, path: "../outside.ts" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
});

test("path restore scopes to a single tool call when toolCallId is given", async () => {
  const { exec, text } = fakeExec({ [`${CWD}/a.ts`]: "v2" });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([restoreSession]),
  );
  const result = await Effect.runPromise(
    cp.restore("s1", { confirm: true, path: "a.ts", toolCallId: "c2" }),
  );
  expect(result.restored[0]?.action).toBe("written");
  expect(text(`${CWD}/a.ts`)).toBe("v1");
});

const REF = "abc123";
const BASE = "def456";

const checkpointSession = session(
  "ck",
  [],
  [{ ref: REF, createdAt: 1, runCount: 2, kind: "stash" }],
);

/** Scripted git answering the checkpoint-restore probe sequence. */
const checkpointGit = (
  covered: string,
  blobs: Record<string, string>,
): NonNullable<Parameters<typeof fakeExec>[1]> => {
  return (_cwd, args) => {
    const cmd = args.join(" ");
    if (cmd === "rev-parse --is-inside-work-tree") return { code: 0, stdout: "true\n" };
    if (cmd === `cat-file -e ${REF}^{commit}`) return { code: 0 };
    if (cmd === "rev-parse --show-toplevel") return { code: 0, stdout: `${CWD}\n` };
    if (cmd === `rev-list --parents -n 1 ${REF}`) return { code: 0, stdout: `${REF} ${BASE}\n` };
    if (cmd === `diff --name-only -z ${BASE} ${REF}`) return { code: 0, stdout: covered };
    for (const [rel, content] of Object.entries(blobs)) {
      if (cmd === `cat-file -e ${REF}:${rel}`) return { code: 0 };
      if (cmd === `show ${REF}:${rel}`) return { code: 0, stdout: content };
    }
    return { code: 128, stderr: `unhandled: ${cmd}` };
  };
};

test("checkpoint restore materializes covered files and deletes removed ones", async () => {
  const { exec, fs } = fakeExec(
    { [`${CWD}/a.ts`]: "newer", [`${CWD}/same.ts`]: "same", [`${CWD}/gone.ts`]: "added later" },
    checkpointGit("a.ts\0same.ts\0gone.ts\0", { "a.ts": "old", "same.ts": "same" }),
  );
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await Effect.runPromise(cp.restore("ck", { confirm: true, checkpoint: REF }));
  expect(result.restored).toEqual([
    { path: `${CWD}/a.ts`, action: "written", bytes: 3 },
    { path: `${CWD}/same.ts`, action: "unchanged" },
    { path: `${CWD}/gone.ts`, action: "deleted" },
  ]);
  expect(result.skipped).toEqual([]);
  expect(fs.has(`${CWD}/gone.ts`)).toBe(false);
});

test("checkpoint restore narrows to requested paths", async () => {
  const { exec } = fakeExec(
    { [`${CWD}/a.ts`]: "newer" },
    checkpointGit("a.ts\0b.ts\0", { "a.ts": "old", "b.ts": "oldb" }),
  );
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await Effect.runPromise(
    cp.restore("ck", { confirm: true, checkpoint: REF, paths: ["a.ts", "unrelated.ts"] }),
  );
  expect(result.restored).toEqual([{ path: `${CWD}/a.ts`, action: "written", bytes: 3 }]);
  expect(result.skipped).toEqual([
    { path: "unrelated.ts", reason: `not touched by checkpoint ${REF}` },
  ]);
});

test("checkpoint restore rejects unknown refs", async () => {
  const { exec } = fakeExec();
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await runEither(cp.restore("ck", { confirm: true, checkpoint: "0000000" }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("not_found");
});

test("checkpoint restore fails when the ref is absent from the workspace repo", async () => {
  const { exec } = fakeExec({}, (_cwd, args) => {
    if (args.join(" ") === "rev-parse --is-inside-work-tree") return { code: 0, stdout: "true\n" };
    return { code: 1, stderr: "missing" };
  });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await runEither(cp.restore("ck", { confirm: true, checkpoint: REF }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("conflict");
});

test("checkpoint restore fails outside a git work tree", async () => {
  const { exec } = fakeExec({}, () => ({ code: 128, stderr: "not a repo" }));
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await runEither(cp.restore("ck", { confirm: true, checkpoint: REF }));
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("conflict");
});

test("checkpoint restore skips files outside the session cwd", async () => {
  const { exec, text } = fakeExec(
    {},
    checkpointGit("a.ts\0other/x.ts\0", { "a.ts": "old", "other/x.ts": "x" }),
  );
  // cwd nested one level under the repo root.
  const withCwd = session("ck2", [], checkpointSession.checkpoints, `${CWD}/sub`);
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([withCwd]),
  );
  const result = await Effect.runPromise(cp.restore("ck2", { confirm: true, checkpoint: REF }));
  expect(result.restored).toEqual([]);
  expect(result.skipped.map((s) => s.reason)).toEqual([
    "outside the session working directory",
    "outside the session working directory",
  ]);
  expect(text(`${CWD}/sub/a.ts`)).toBeNull();
});

test("checkpoint restore reports a git show failure per file", async () => {
  const { exec } = fakeExec({ [`${CWD}/a.ts`]: "newer" }, (cwd, args) => {
    const base = checkpointGit("a.ts\0", { "a.ts": "x" })(cwd, args);
    if (args.join(" ") === `show ${REF}:a.ts`) return { code: 1, stderr: "boom" };
    return base;
  });
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await Effect.runPromise(cp.restore("ck", { confirm: true, checkpoint: REF }));
  expect(result.skipped).toEqual([{ path: "a.ts", reason: "git show failed: boom" }]);
});

test("checkpoint restore rejects path escapes in the paths subset", async () => {
  const { exec } = fakeExec({}, checkpointGit("a.ts\0", { "a.ts": "old" }));
  const cp = await makeService(
    { agents: [fakeAgent(new FakeConnection())], restoreExec: exec },
    repository([checkpointSession]),
  );
  const result = await runEither(
    cp.restore("ck", { confirm: true, checkpoint: REF, paths: ["../evil.ts"] }),
  );
  expect(Either.isLeft(result)).toBe(true);
  if (Either.isLeft(result)) expect(result.left.code).toBe("invalid");
});
