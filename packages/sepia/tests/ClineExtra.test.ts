/**
 * Cline.ts edge coverage — `mapToolUse` raw fallbacks, `toolResultShares`
 * alignment paths, manifest/messages degenerate inputs, and the writer-side
 * tool-name/result mappings `sessionMessages` uses.
 */
import * as FileSystem from "@effect/platform/FileSystem";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Path from "@effect/platform/Path";
import { Effect, Either, Layer, Option } from "effect";
import { expect, test, vi } from "vite-plus/test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as Cline from "../src/Cline.js";
import { MessageNode, Session, ToolCall } from "../src/Domain.js";

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const withTempDir = async <T>(prefix: string, fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const writeClineDir = (
  root: string,
  id: string,
  messages: ReadonlyArray<unknown>,
  manifest: Record<string, unknown> = {},
  messagesContent?: string,
): string => {
  const dir = join(root, id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, `${id}.messages.json`),
    messagesContent ?? JSON.stringify({ version: 1, sessionId: id, messages }),
  );
  writeFileSync(
    join(dir, `${id}.json`),
    JSON.stringify({
      version: 1,
      session_id: id,
      cwd: "/work",
      started_at: "2026-01-01T00:00:00.000Z",
      ended_at: "2026-01-01T00:00:05.000Z",
      status: "completed",
      model: "glm-5-2",
      ...manifest,
    }),
  );
  return dir;
};

const load = (root: string, id: string): Promise<Session> =>
  Effect.runPromise(Cline.fromDirectory(join(root, id)).pipe(Effect.provide(fsLayer)));

const loadEither = (root: string, id: string) =>
  Effect.runPromise(
    Effect.either(Cline.fromDirectory(join(root, id)).pipe(Effect.provide(fsLayer))),
  );

/* ---- mapToolUse raw fallbacks ----------------------------------------- */

test("import keeps tool calls whose list fields are empty or absent", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [
      { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: 1000 },
      {
        id: "m1",
        role: "assistant",
        content: [
          { type: "tool_use", id: "t1", name: "read_files", input: {} },
          { type: "tool_use", id: "t2", name: "search_codebase", input: { queries: [] } },
          { type: "tool_use", id: "t3", name: "fetch_web_content", input: {} },
          { type: "tool_use", id: "t4", name: "editor", input: { path: "" } },
          { type: "tool_use", id: "t5", name: "run_commands" },
        ],
        ts: 2000,
      },
    ]);
    const session = await load(root, "s1");
    const assistant = session.nodes.find((n) => n.role === "assistant" && n.toolCalls.length > 0);
    expect(assistant).toBeDefined();
    const names = assistant!.toolCalls.map((tc) => tc.name);
    // none of these split into calls — each survives verbatim, name and all
    expect(names).toEqual([
      "read_files",
      "search_codebase",
      "fetch_web_content",
      "editor",
      "run_commands",
    ]);
    expect(assistant!.toolCalls[4]!.arguments).toEqual({});
  }));

test("import maps fetch_web_content requests to webfetch calls", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [
      { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: 1000 },
      {
        id: "m1",
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "t1",
            name: "fetch_web_content",
            input: { requests: ["https://a.io", { url: "https://b.io" }] },
          },
          // a search whose queries arrive as a nested array
          {
            type: "tool_use",
            id: "t2",
            name: "search_codebase",
            input: { queries: [["inner", { pattern: "obj" }]] },
          },
          // a read whose file entry carries no path-like key stringifies whole
          { type: "tool_use", id: "t3", name: "read_files", input: { files: [{ odd: 1 }] } },
          // a null entry resolves to "" — a call without a location
          { type: "tool_use", id: "t4", name: "read_files", input: { files: [null] } },
          // a bare number stringifies
          { type: "tool_use", id: "t5", name: "read_files", input: { files: [42] } },
        ],
        ts: 2000,
      },
    ]);
    const session = await load(root, "s1");
    const assistant = session.nodes.find((n) => n.role === "assistant" && n.toolCalls.length > 0)!;
    const [fetch1, fetch2, grep1, grep2, read1, read2, read3] = assistant.toolCalls;
    expect(fetch1).toMatchObject({ name: "webfetch", arguments: { url: "https://a.io" } });
    expect(fetch2).toMatchObject({ name: "webfetch", arguments: { url: "https://b.io" } });
    expect(grep1).toMatchObject({ name: "grep", arguments: { pattern: "inner" } });
    expect(grep2).toMatchObject({ name: "grep", arguments: { pattern: "obj" } });
    expect(read1!.arguments).toEqual({ file_path: '{"odd":1}' });
    // no readable path → the call stays but records no location
    expect(read2!.name).toBe("read");
    expect(read2!.arguments).toEqual({ file_path: "" });
    expect(read2!.locations).toEqual([]);
    expect(read3!.arguments).toEqual({ file_path: "42" });
  }));

test("import folds editor inputs into write/edit calls with diff records", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [
      { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: 1000 },
      {
        id: "m1",
        role: "assistant",
        content: [
          // create: old_text "null" is the CLI's marker for a new file
          {
            type: "tool_use",
            id: "t1",
            name: "editor",
            input: { path: "/work/a.ts", old_text: "null", new_text: "body" },
          },
          // create with a non-string payload keeps the path marker only
          {
            type: "tool_use",
            id: "t2",
            name: "editor",
            input: { path: "/work/b.ts", old_text: null, new_text: { x: 1 } },
          },
          // edit with non-string hunks serializes them
          {
            type: "tool_use",
            id: "t3",
            name: "editor",
            input: { path: "/work/c.ts", old_text: { o: 1 }, new_text: 7 },
          },
        ],
        ts: 2000,
      },
    ]);
    const session = await load(root, "s1");
    const assistant = session.nodes.find((n) => n.role === "assistant" && n.toolCalls.length > 0)!;
    const [create, oddCreate, edit] = assistant.toolCalls;
    expect(create).toMatchObject({
      name: "write",
      arguments: { file_path: "/work/a.ts", content: "body" },
      diffs: [{ path: "/work/a.ts", newText: "body" }],
    });
    expect(oddCreate).toMatchObject({ name: "write", diffs: [{ path: "/work/b.ts" }] });
    expect(edit).toMatchObject({
      name: "edit",
      diffs: [{ path: "/work/c.ts", oldText: '{"o":1}' }],
    });
  }));

/* ---- toolResultShares -------------------------------------------------- */

test("import distributes a non-text, non-list result body across the calls", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [
      { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: 1000 },
      {
        id: "m1",
        role: "assistant",
        content: [
          { type: "tool_use", id: "t1", name: "run_commands", input: { commands: ["ls"] } },
        ],
        ts: 2000,
      },
      // the result body is a bare number — every call's share serializes it
      {
        id: "m2",
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "t1", content: 42 }],
        ts: 3000,
      },
      {
        id: "m3",
        role: "assistant",
        content: [
          { type: "tool_use", id: "t9", name: "run_commands", input: { commands: ["pwd"] } },
        ],
        ts: 4000,
      },
      {
        id: "m4",
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "t9" }],
        ts: 5000,
      },
    ]);
    const session = await load(root, "s1");
    const tools = session.nodes.filter((n) => n.role === "tool");
    expect(tools[0]!.content).toBe("42");
    // no content at all → empty output, still a success
    expect(tools[1]!.content).toBe("");
    expect(Option.getOrUndefined(tools[1]!.toolResult)?.status).toBe("success");
  }));

test("import keeps answer parts that match by url or arrive as bare strings", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [
      { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: 1000 },
      {
        id: "m1",
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "t1",
            name: "fetch_web_content",
            input: { requests: ["https://a.io", "https://b.io"] },
          },
        ],
        ts: 2000,
      },
      {
        id: "m2",
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "t1",
            content: [
              // keyed by url rather than query
              { url: "https://b.io", result: "page b", success: false },
              // a bare string entry claims nothing — it lands in the tail
              "loose text",
              // an object with neither query/url/result stringifies whole
              { meta: true },
              // an array query — keyOf joins it
              { query: ["q1", "q2"], result: "joined" },
            ],
          },
        ],
        ts: 3000,
      },
    ]);
    const session = await load(root, "s1");
    const tools = session.nodes.filter((n) => n.role === "tool");
    expect(tools).toHaveLength(2);
    // the bare string entry fills the call no key matched (a.io)
    expect(tools[0]!.content).toBe("loose text");
    // https://b.io matched the second call by url key; the leftovers
    // (keyless object, unmatched array query) merge onto the last call's
    // share, and the recorded failure marks it.
    const last = tools[1]!;
    expect(last.content).toContain("page b");
    expect(last.content).toContain('{"meta":true}');
    expect(last.content).toContain("joined");
    expect(Option.getOrUndefined(last.toolResult)?.status).toBe("error");
  }));

/* ---- manifest/messages degenerate inputs ------------------------------ */

test("fromDirectory reports missing dirs, missing manifests and bad builds", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    const missing = await loadEither(root, "ghost");
    expect(Either.isLeft(missing)).toBe(true);
    if (Either.isLeft(missing)) expect(missing.left.message).toContain("not found");

    // a dir with only a .messages.json — no manifest
    const bare = join(root, "bare");
    mkdirSync(bare);
    writeFileSync(join(bare, "bare.messages.json"), "{}");
    const noManifest = await loadEither(root, "bare");
    expect(Either.isLeft(noManifest)).toBe(true);
    if (Either.isLeft(noManifest)) expect(noManifest.left.message).toContain("metadata json");

    // buildSession blows up on a non-string model
    writeClineDir(root, "badbuild", [], { model: 5 });
    const badBuild = await loadEither(root, "badbuild");
    expect(Either.isLeft(badBuild)).toBe(true);
    if (Either.isLeft(badBuild)) expect(badBuild.left.message).toContain("Failed to build session");

    // manifest parses but the transcript is missing → raw fs error wraps
    const dir = join(root, "nomsg");
    mkdirSync(dir);
    writeFileSync(join(dir, "nomsg.json"), JSON.stringify({ session_id: "nomsg" }));
    const noMsg = await loadEither(root, "nomsg");
    expect(Either.isLeft(noMsg)).toBe(true);
    if (Either.isLeft(noMsg)) expect(noMsg.left.message).toContain("Cline conversion failed");

    // the messages file is valid JSON but not an object → no messages
    writeClineDir(root, "num", [], {}, "5");
    const num = await load(root, "num");
    // two system nodes + an empty user node is all a message-less log yields
    expect(num.nodes.every((n) => n.role !== "assistant")).toBe(true);
  }));

test("fromDirectory tolerates missing meta fields and non-numeric timestamps", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    // no session_id → dir name; no cwd → dir; no title → prompt → fallback
    writeClineDir(
      root,
      "sparse",
      [
        // non-numeric ts falls back to the session start
        { id: "m0", role: "user", content: [{ type: "text", text: "go" }], ts: "not-a-ts" },
        { id: "m1", role: "assistant", content: [{ type: "text", text: "ok" }] },
      ],
      { session_id: undefined as unknown as string, cwd: undefined as unknown as string },
    );
    // manifest without session_id: rewrite it bare
    const dir = join(root, "sparse");
    writeFileSync(join(dir, "sparse.json"), JSON.stringify({ version: 1, status: "done" }));
    const session = await load(root, "sparse");
    expect(session.id).toBe("sparse");
    expect(session.title).toBe("Imported session");
    expect(session.workingDirectory).toBe(dir);
    expect(session.createdAt).toBe(session.nodes[0]!.createdAt);
    expect(session.promptHistory[0]!.timestamp).toBe(session.createdAt * 1000);
  }));

test("checkpointsFromManifest drops malformed entries and keeps a lone latest", async () =>
  withTempDir("sepia-cline-extra-", async (root) => {
    writeClineDir(root, "s1", [], {
      metadata: {
        checkpoint: {
          history: [
            { ref: "aaa", createdAt: 1000, runCount: 1, kind: "stash" },
            { ref: "bbb", createdAt: "not a number" },
            { ref: undefined, createdAt: 5 },
            "junk",
            { ref: "ccc", createdAt: 3000, runCount: "x" },
          ],
          latest: { ref: "newest", createdAt: 4000 },
        },
      },
    });
    const session = await load(root, "s1");
    expect(session.checkpoints).toEqual([
      { ref: "aaa", createdAt: 1000, runCount: 1, kind: "stash" },
      { ref: "ccc", createdAt: 3000 },
      { ref: "newest", createdAt: 4000 },
    ]);

    // latest already in history → not duplicated; non-list history → []
    writeClineDir(root, "s2", [], {
      metadata: { checkpoint: { history: "none", latest: { ref: "l", createdAt: 1 } } },
    });
    expect((await load(root, "s2")).checkpoints).toEqual([{ ref: "l", createdAt: 1 }]);

    writeClineDir(root, "s3", [], {
      metadata: {
        checkpoint: {
          history: [{ ref: "only", createdAt: 1 }],
          latest: { ref: "only", createdAt: 1 },
        },
      },
    });
    expect((await load(root, "s3")).checkpoints).toEqual([{ ref: "only", createdAt: 1 }]);
  }));

/* ---- sessionMessages writer paths -------------------------------------- */

const rendered = { summarized_from: null, num_tokens_preceding: 12, is_system_prefix: null };

const wNode = (input: {
  nodeId: number;
  role: MessageNode["role"];
  content: string;
  toolCalls?: ReadonlyArray<ToolCall>;
  toolCallId?: Option.Option<string>;
  toolName?: Option.Option<string>;
  thinking?: string;
  thinkingSignature?: string;
  metadata?: unknown;
}): MessageNode =>
  MessageNode.make({
    nodeId: input.nodeId,
    parentNodeId: input.nodeId === 0 ? Option.none() : Option.some(input.nodeId - 1),
    role: input.role,
    content: input.content,
    toolCalls: input.toolCalls ?? [],
    toolCallId: input.toolCallId ?? Option.none(),
    toolName: input.toolName ?? Option.none(),
    thinking: Option.fromNullable(input.thinking),
    thinkingSignature: Option.fromNullable(input.thinkingSignature),
    createdAt: 1_700_000_000 + input.nodeId,
    metadata: input.metadata ?? null,
  });

const wSession = (nodes: ReadonlyArray<MessageNode>): Session =>
  Session.make({
    id: "s",
    title: "t",
    workingDirectory: "/work",
    model: "m",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_010,
    mainChainId: nodes.length - 1,
    metadata: null,
    nodes,
  });

test("sessionMessages maps webfetch/edit/write calls back to Cline inputs", () => {
  const assistant = wNode({
    nodeId: 1,
    role: "assistant",
    content: "working",
    toolCalls: [
      ToolCall.make({ id: "w1", name: "webfetch", arguments: { url: "https://a.io" } }),
      ToolCall.make({
        id: "e1",
        name: "edit",
        arguments: { file_path: "/a.ts", old_string: "o", new_string: "n" },
      }),
      ToolCall.make({ id: "wr1", name: "write", arguments: { file_path: "/b.ts", content: "c" } }),
    ],
    metadata: rendered,
  });
  const out = Cline.sessionMessages(
    wSession([wNode({ nodeId: 0, role: "user", content: "go" }), assistant]),
    "s",
  );
  const assistantMsg = (out.messages as Array<any>).find((m) => m.role === "assistant");
  const uses = assistantMsg.content.filter((b: any) => b.type === "tool_use");
  expect(uses).toEqual([
    {
      type: "tool_use",
      id: "w1",
      name: "fetch_web_content",
      input: { requests: [{ url: "https://a.io" }] },
    },
    {
      type: "tool_use",
      id: "e1",
      name: "editor",
      input: { path: "/a.ts", old_text: "o", new_text: "n" },
    },
    {
      type: "tool_use",
      id: "wr1",
      name: "editor",
      input: { path: "/b.ts", old_text: null, new_text: "c" },
    },
  ]);
  // the calls have no recorded results — placeholders keep the log valid
  expect(Cline.transcriptViolations(out.messages as ReadonlyArray<unknown>)).toEqual([]);
});

test("sessionMessages writes orphaned results and signature-only thinking", () => {
  const messages = Cline.sessionMessages(
    wSession([
      wNode({ nodeId: 0, role: "user", content: "go" }),
      // a tool node no assistant turn claims — lands as its own user entry
      wNode({ nodeId: 1, role: "tool", content: "late output" }),
      // an orphan naming a tool still carries it
      wNode({
        nodeId: 2,
        role: "tool",
        content: "grep out",
        toolCallId: Option.some("gone"),
        toolName: Option.some("grep"),
        metadata: { toolArguments: { pattern: "x" } },
      }),
      wNode({
        nodeId: 3,
        role: "assistant",
        content: "",
        // a seal without text is a fully redacted block
        thinkingSignature: "opaque-blob",
      }),
    ]),
    "s",
  );
  const list = messages.messages as Array<any>;
  const orphan = list[1];
  expect(orphan.role).toBe("user");
  expect(orphan.content[0]).toMatchObject({
    type: "tool_result",
    tool_use_id: "",
    name: "unknown",
    content: "late output",
  });
  const named = list[2];
  // a named call whose tool maps back gets the keyed result envelope
  expect(named.content[0]).toMatchObject({
    type: "tool_result",
    tool_use_id: "gone",
    name: "search_codebase",
    content: [{ query: "x", result: "grep out", success: true }],
  });
  const assistant = list.find((m) => m.role === "assistant");
  expect(assistant.content).toEqual([{ type: "redacted_thinking", data: "opaque-blob" }]);
});

test("sessionMessages keeps results for unmapped tool names as plain content", () => {
  const messages = Cline.sessionMessages(
    wSession([
      wNode({ nodeId: 0, role: "user", content: "go" }),
      wNode({
        nodeId: 1,
        role: "assistant",
        content: "ran it",
        toolCalls: [ToolCall.make({ id: "b1", name: "Bash", arguments: { command: "ls" } })],
      }),
      // the result node claims the call but names a different, unmapped tool
      wNode({
        nodeId: 2,
        role: "tool",
        content: "done",
        toolCallId: Option.some("b1"),
        toolName: Option.none(),
      }),
    ]),
    "s",
  );
  const list = messages.messages as Array<any>;
  const results = list.find((m) => m.role === "user" && m.id !== "msg_0");
  // toolName absent → the call's own name stands in; "Bash" is not a keyed
  // result tool so the body is the raw content
  expect(results.content[0]).toMatchObject({
    type: "tool_result",
    tool_use_id: "b1",
    name: "Bash",
    content: "done",
  });
});

test("toDirectory reports kept/planned/replaced/created and write failures", async () => {
  const session = wSession([wNode({ nodeId: 0, role: "user", content: "hi" })]);

  // kept: a manifest already sits in the out dir
  const existing = new Map<string, string>([["/out/s.json", "{}"]]);
  const keptFs = FileSystem.layerNoop({
    exists: (path) => Effect.succeed(existing.has(path)),
    readFileString: (path) =>
      existing.has(path)
        ? Effect.succeed(existing.get(path)!)
        : Effect.fail({ message: "nope" } as never),
  });
  const spy = vi.spyOn(console, "log").mockImplementation(() => {});
  try {
    const kept = await Effect.runPromise(
      Cline.toDirectory(session, "/out").pipe(Effect.provide(Layer.merge(keptFs, Path.layer))),
    );
    expect(kept).toEqual(["kept", "kept"]);
    expect(spy).toHaveBeenCalledWith(expect.stringContaining("already exists"));
  } finally {
    spy.mockRestore();
  }

  // planned: dryRun with nothing on disk writes nothing
  const written = new Map<string, string>();
  const dryFs = FileSystem.layerNoop({
    exists: () => Effect.succeed(false),
    makeDirectory: () => Effect.void,
    writeFileString: (path, data) => Effect.sync(() => void written.set(path, data)),
  });
  const spy2 = vi.spyOn(console, "log").mockImplementation(() => {});
  try {
    const planned = await Effect.runPromise(
      Cline.toDirectory(session, "/dry", { dryRun: true }).pipe(
        Effect.provide(Layer.merge(dryFs, Path.layer)),
      ),
    );
    expect(planned).toEqual(["planned", "planned"]);
    expect(written.size).toBe(0);
    expect(spy2).toHaveBeenCalledWith(expect.stringContaining("Would export"));
  } finally {
    spy2.mockRestore();
  }

  // replaced: force over an existing manifest
  const replaceFs = FileSystem.layerNoop({
    exists: (path) => Effect.succeed(path === "/out/s.json"),
    readFileString: () => Effect.succeed("{}"),
    makeDirectory: () => Effect.void,
    writeFileString: (path, data) => Effect.sync(() => void written.set(path, data)),
  });
  const replaced = await Effect.runPromise(
    Cline.toDirectory(session, "/out", { force: true }).pipe(
      Effect.provide(Layer.merge(replaceFs, Path.layer)),
    ),
  );
  expect(replaced).toEqual(["replaced", "replaced"]);

  // write failure surfaces as a ConversionError
  const failFs = FileSystem.layerNoop({
    exists: () => Effect.succeed(false),
    makeDirectory: () => Effect.void,
    writeFileString: () => Effect.fail({ message: "disk full" } as never),
  });
  await expect(
    Effect.runPromise(
      Cline.toDirectory(session, "/x").pipe(Effect.provide(Layer.merge(failFs, Path.layer))),
    ),
  ).rejects.toThrow("Cline export failed");
});

test("readSessionFiles reads what exists and tolerates a missing transcript", async () => {
  // readSessionFiles takes the FileSystem *service*, not a layer
  const fs = FileSystem.makeNoop({
    exists: (path) => Effect.succeed(path === "/out/s.json"),
    readFileString: (path) =>
      path === "/out/s.json"
        ? Effect.succeed('{"a":1}')
        : Effect.fail({ message: "missing" } as never),
  });
  const found = await Effect.runPromise(Cline.readSessionFiles(fs, "s", "/out"));
  expect(Option.getOrThrow(found)).toEqual({ manifest: '{"a":1}', messages: "{}" });

  // nothing on disk → none
  const missing = await Effect.runPromise(
    Cline.readSessionFiles(
      FileSystem.makeNoop({ exists: () => Effect.succeed(false) }),
      "s",
      "/out",
    ),
  );
  expect(Option.isNone(missing)).toBe(true);
});

test("transcriptViolations flags unresolved calls and stray messages", () => {
  const violations = Cline.transcriptViolations([
    { role: "assistant", content: [{ type: "tool_use", id: "c1" }] },
    // a text user message while c1 is pending — the CLI rejects this
    { role: "user", content: [{ type: "text", text: "interrupt" }] },
    // a result-only user message resolves c1, leaves c2 open
    {
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "c1", content: "done" },
        { type: "tool_result", tool_use_id: "c1b", content: "x" },
      ],
    },
    { role: "assistant", content: [{ type: "tool_use", id: "c2" }] },
    // non-message junk contributes nothing
    "junk",
    { role: "assistant", content: "not-a-list" },
  ]);
  expect(violations).toEqual([
    { index: 1, toolCallIds: ["c1"] },
    // a non-message line is not a result-only message — it trips the
    // still-pending c2 as well
    { index: 4, toolCallIds: ["c2"] },
    { index: "eof", toolCallIds: ["c2"] },
  ]);
});

test("readSessionFiles wraps a failed manifest read as a ConversionError", async () => {
  const fs = FileSystem.makeNoop({
    exists: () => Effect.succeed(true),
    readFileString: () => Effect.fail({ message: "io error" } as never),
  });
  await expect(Effect.runPromise(Cline.readSessionFiles(fs, "s", "/out"))).rejects.toThrow(
    "Cline export failed",
  );
});
