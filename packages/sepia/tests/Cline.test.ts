import * as FileSystem from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Effect, Layer, Option } from "effect";
import { expect, test } from "vite-plus/test";
import * as Cline from "../src/Cline.js";
import * as ClineIndex from "../src/ClineIndex.js";
import { MessageNode, Session, ToolCall } from "../src/Domain.js";
import { needsMigration } from "../src/Storage.js";

const makeNode = (input: {
  nodeId: number;
  parentNodeId?: Option.Option<number>;
  role: MessageNode["role"];
  content: string;
  toolCalls?: ReadonlyArray<ToolCall>;
  toolCallId?: Option.Option<string>;
  toolName?: Option.Option<string>;
  toolResult?: Option.Option<import("../src/Domain.js").ToolResultInfo>;
  thinking?: string;
  thinkingSignature?: string;
  metadata?: unknown;
}): MessageNode =>
  MessageNode.make({
    nodeId: input.nodeId,
    parentNodeId: input.parentNodeId ?? Option.none(),
    role: input.role,
    content: input.content,
    toolCalls: input.toolCalls ?? [],
    toolCallId: input.toolCallId ?? Option.none(),
    toolName: input.toolName ?? Option.none(),
    toolResult: input.toolResult ?? Option.none(),
    thinking: Option.fromNullable(input.thinking),
    thinkingSignature: Option.fromNullable(input.thinkingSignature),
    createdAt: 1700000000 + input.nodeId,
    metadata: input.metadata ?? null,
  });

const makeSession = (nodes: ReadonlyArray<MessageNode>): Session =>
  Session.make({
    id: "imported-session",
    title: "Imported session",
    workingDirectory: "/work",
    model: "swe-2-high",
    createdAt: 1700000000,
    lastActivityAt: 1700000300,
    mainChainId: nodes.length,
    metadata: null,
    nodes,
  });

const rendered = { summarized_from: null, num_tokens_preceding: 12, is_system_prefix: null };

const sampleNodes = (): ReadonlyArray<MessageNode> => [
  makeNode({ nodeId: 0, role: "system", content: "system prompt" }),
  makeNode({ nodeId: 1, role: "user", content: "do the thing" }),
  makeNode({
    nodeId: 2,
    parentNodeId: Option.some(1),
    role: "assistant",
    content: "working on it",
    toolCalls: [
      ToolCall.make({ id: "call_1", name: "read", arguments: { file_path: "/work/a.ts" } }),
    ],
    metadata: rendered,
  }),
  makeNode({
    nodeId: 3,
    parentNodeId: Option.some(2),
    role: "tool",
    content: "file body",
    toolCallId: Option.some("call_1"),
    toolName: Option.some("read"),
    metadata: { toolArguments: { file_path: "/work/a.ts" } },
  }),
  makeNode({
    nodeId: 4,
    parentNodeId: Option.some(3),
    role: "assistant",
    content: "done",
    metadata: rendered,
  }),
  makeNode({
    nodeId: 5,
    parentNodeId: Option.some(4),
    role: "assistant",
    content: "next step",
    metadata: null,
  }),
  makeNode({
    nodeId: 6,
    parentNodeId: Option.some(4),
    role: "assistant",
    content: "next step",
    metadata: rendered,
  }),
];

test("visibleNodes keeps assistant turns and collapses rendered twins", () => {
  const visible = Cline.visibleNodes(makeSession(sampleNodes()));

  // system node dropped, rendered twin of node 5 (node 6) dropped.
  expect(visible.map((node) => node.nodeId)).toEqual([1, 2, 3, 4, 5]);
});

test("toDirectory exports a Cline session that resumes with assistant turns intact", async () => {
  const files = new Map<string, string>();
  const fsLayer = FileSystem.layerNoop({
    makeDirectory: () => Effect.void,
    writeFileString: (path, data) =>
      Effect.sync(() => {
        files.set(path, data);
      }),
  });

  const session = makeSession(sampleNodes());
  await Effect.runPromise(
    Cline.toDirectory(session, "/out").pipe(Effect.provide(Layer.merge(fsLayer, Path.layer))),
  );

  const meta = JSON.parse(files.get("/out/imported-session.json") ?? "{}");
  const data = JSON.parse(files.get("/out/imported-session.messages.json") ?? "{}");
  const messages = data.messages as ReadonlyArray<any>;

  // manifest carries every field the Cline CLI requires to resume by id.
  expect(meta.session_id).toBe("imported-session");
  expect(meta.cwd).toBe("/work");
  expect(meta.workspace_root).toBe("/work");
  expect(meta.status).toBe("completed");
  expect(meta.pid).toBe(0);
  expect(meta.enable_tools).toBe(true);
  expect(meta.enable_spawn).toBe(true);
  expect(meta.enable_teams).toBe(true);
  expect(meta.prompt).toBe("do the thing");
  expect(meta.metadata.title).toBe("Imported session");
  expect(meta.messages_path).toBe("/out/imported-session.messages.json");

  expect(data.sessionId).toBe("imported-session");
  expect(data.messages.length).toBe(5);
  expect(messages.map((m) => m.role)).toEqual([
    "user",
    "assistant",
    "user",
    "assistant",
    "assistant",
  ]);

  // assistant text survives (previously every assistant turn was dropped) …
  expect(messages[3].content).toEqual([{ type: "text", text: "done" }]);
  expect(messages[3].modelInfo).toEqual({ id: "swe-2-high", provider: "cline-pass" });

  // … and tool calls stay paired with their results.
  expect(messages[1].content[1]).toEqual({
    type: "tool_use",
    id: "call_1",
    name: "read_files",
    input: { files: [{ path: "/work/a.ts" }] },
  });
  expect(messages[2].content).toEqual([
    {
      type: "tool_result",
      tool_use_id: "call_1",
      name: "read_files",
      content: [{ query: "/work/a.ts", result: "file body", success: true }],
    },
  ]);

  // no turn may land between a tool call and its result: the CLI's replay of
  // the log rejects that with AI_MissingToolResultsError.
  expect(Cline.transcriptViolations(messages)).toEqual([]);
});

test("sessionMessages pairs a call whose result never arrived with a placeholder", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "user", content: "do the thing" }),
    makeNode({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "assistant",
      content: "on it",
      toolCalls: [
        ToolCall.make({ id: "call_1", name: "read", arguments: { file_path: "/work/a.ts" } }),
        ToolCall.make({ id: "call_2", name: "exec", arguments: { command: "pwd" } }),
      ],
      metadata: rendered,
    }),
    makeNode({
      nodeId: 2,
      parentNodeId: Option.some(1),
      role: "tool",
      content: "file body",
      toolCallId: Option.some("call_1"),
      toolName: Option.some("read"),
      metadata: { toolArguments: { file_path: "/work/a.ts" } },
    }),
    makeNode({
      nodeId: 3,
      parentNodeId: Option.some(2),
      role: "assistant",
      content: "done",
      metadata: rendered,
    }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;

  // the interrupted call must not leave every later user turn unresolved.
  expect(Cline.transcriptViolations(messages)).toEqual([]);
  expect(messages.map((m) => m.role)).toEqual(["user", "assistant", "user", "assistant"]);
  expect(messages[2].content).toEqual([
    {
      type: "tool_result",
      tool_use_id: "call_1",
      name: "read_files",
      content: [{ query: "/work/a.ts", result: "file body", success: true }],
    },
    {
      type: "tool_result",
      tool_use_id: "call_2",
      name: "run_commands",
      content: Cline.UNCAPTURED_TOOL_RESULT,
    },
  ]);
});

test("sessionMessages hoists a result the store recorded behind a user turn", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "user", content: "do the thing" }),
    makeNode({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "assistant",
      content: "",
      toolCalls: [
        ToolCall.make({ id: "call_1", name: "read", arguments: { file_path: "/work/a.ts" } }),
      ],
      metadata: rendered,
    }),
    makeNode({ nodeId: 2, parentNodeId: Option.some(1), role: "user", content: "continue" }),
    makeNode({
      nodeId: 3,
      parentNodeId: Option.some(1),
      role: "tool",
      content: "file body",
      toolCallId: Option.some("call_1"),
      toolName: Option.some("read"),
      metadata: { toolArguments: { file_path: "/work/a.ts" } },
    }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;

  // the result is paired by tool-call id, not by stored position, so the
  // interrupting user turn cannot land while the call is unresolved.
  expect(messages.map((m) => m.role)).toEqual(["user", "assistant", "user", "user"]);
  expect(messages[2].content).toEqual([
    {
      type: "tool_result",
      tool_use_id: "call_1",
      name: "read_files",
      content: [{ query: "/work/a.ts", result: "file body", success: true }],
    },
  ]);
  expect(messages[3].content).toEqual([{ type: "text", text: "continue" }]);
  expect(Cline.transcriptViolations(messages)).toEqual([]);
});

test("sessionMessages writes thinking seals back verbatim", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "user", content: "go" }),
    makeNode({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "assistant",
      content: "done",
      thinking: "ponder",
      thinkingSignature: "sealed.v1.sig",
      metadata: rendered,
    }),
    makeNode({
      nodeId: 2,
      parentNodeId: Option.some(1),
      role: "assistant",
      content: "ok",
      thinking: "[redacted]",
      thinkingSignature: "opaque-blob",
      metadata: rendered,
    }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;
  const assistants = messages.filter((m) => m.role === "assistant");

  expect(assistants[0].content).toContainEqual({
    type: "thinking",
    thinking: "ponder",
    signature: "sealed.v1.sig",
  });
  // a marker-only thinking + opaque blob was a redacted block — write it as one
  expect(assistants[1].content).toContainEqual({
    type: "redacted_thinking",
    data: "opaque-blob",
  });
});

const readOnlyFs = (files: Record<string, string>) =>
  FileSystem.layerNoop({
    exists: (path) =>
      Effect.succeed(
        Object.keys(files).some((file) => file === path || file.startsWith(`${path}/`)),
      ),
    readDirectory: (path) =>
      Effect.succeed([
        ...new Set(
          Object.keys(files)
            .filter((file) => file.startsWith(`${path}/`))
            .map((file) => file.slice(path.length + 1).split("/")[0]),
        ),
      ]),
    readFileString: (path) => Effect.succeed(files[path] ?? ""),
  });

const importedSession = (
  id: string,
  messages: ReadonlyArray<unknown>,
  manifest: Record<string, unknown> = {},
): Promise<Session> =>
  Effect.runPromise(
    Cline.fromDirectory(`/session/${id}`).pipe(
      Effect.provide(
        Layer.merge(
          readOnlyFs({
            [`/session/${id}/${id}.json`]: JSON.stringify({
              session_id: id,
              cwd: "/work",
              model: "cline-pass/swe-2-high",
              started_at: "2026-09-01T00:00:00.000Z",
              ended_at: "2026-09-01T00:01:00.000Z",
              metadata: { title: "imported" },
              ...manifest,
            }),
            [`/session/${id}/${id}.messages.json`]: JSON.stringify({ version: 1, messages }),
          }),
          Path.layer,
        ),
      ),
    ),
  );

const toolResultContents = (session: Session): ReadonlyArray<string> =>
  Cline.visibleNodes(session)
    .filter((node) => node.role === "tool")
    .map((node) => node.content);

test("import records editor diffs, touched-file locations and manifest checkpoints", async () => {
  const session = await importedSession(
    "checkpointed",
    [
      { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
      {
        id: "a0",
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "call_e",
            name: "editor",
            input: { path: "/work/a.ts", old_text: "before", new_text: "after" },
          },
          {
            type: "tool_use",
            id: "call_w",
            name: "editor",
            input: { path: "/work/b.ts", new_text: "body" },
          },
          {
            type: "tool_use",
            id: "call_r",
            name: "read_files",
            input: { files: [{ path: "/work/c.ts" }] },
          },
        ],
        ts: 2,
      },
      {
        id: "u1",
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "call_e", name: "editor", content: "ok" },
          { type: "tool_result", tool_use_id: "call_w", name: "editor", content: "ok" },
          {
            type: "tool_result",
            tool_use_id: "call_r",
            name: "read_files",
            content: [{ query: "/work/c.ts", result: "body", success: true }],
          },
        ],
        ts: 3,
      },
    ],
    {
      metadata: {
        title: "imported",
        checkpoint: {
          latest: { ref: "bbb", createdAt: 200, runCount: 2, kind: "commit" },
          history: [{ ref: "aaa", createdAt: 100, runCount: 1, kind: "stash" }, "junk"],
        },
      },
    },
  );

  // `latest` isn't in this manifest's history — it is appended, not lost.
  expect(session.checkpoints).toEqual([
    { ref: "aaa", createdAt: 100, runCount: 1, kind: "stash" },
    { ref: "bbb", createdAt: 200, runCount: 2, kind: "commit" },
  ]);

  const calls = Cline.visibleNodes(session).flatMap((node) => node.toolCalls);
  const edit = calls.find((call) => call.name === "edit");
  expect(edit?.locations).toEqual([{ path: "/work/a.ts" }]);
  expect(edit?.diffs).toEqual([{ path: "/work/a.ts", oldText: "before", newText: "after" }]);

  // A create (`old_text` absent) records a newText-only diff.
  const write = calls.find((call) => call.name === "write");
  expect(write?.diffs).toEqual([{ path: "/work/b.ts", newText: "body" }]);
  expect(write?.locations).toEqual([{ path: "/work/b.ts" }]);

  const read = calls.find((call) => call.name === "read");
  expect(read?.locations).toEqual([{ path: "/work/c.ts" }]);
  expect(read?.diffs).toEqual([]);

  // The manifest write-back restores the Cline-native {latest, history} blob.
  const manifest = Cline.sessionManifest(session, "checkpointed", "/m") as {
    metadata: { checkpoint: { latest: { ref: string }; history: ReadonlyArray<{ ref: string }> } };
  };
  expect(manifest.metadata.checkpoint.history.map((entry) => entry.ref)).toEqual(["aaa", "bbb"]);
  expect(manifest.metadata.checkpoint.latest.ref).toBe("bbb");

  // Sessions that never checkpointed write no checkpoint metadata at all.
  const plain = Cline.sessionManifest(makeSession(sampleNodes()), "plain", "/m") as {
    metadata: Record<string, unknown>;
  };
  expect(plain.metadata.checkpoint).toBeUndefined();
});

test("import keeps thinking signatures and redacted blobs verbatim", async () => {
  const session = await importedSession("sealed", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        { type: "thinking", thinking: "ponder", signature: "sealed.v1.sig" },
        { type: "text", text: "done" },
      ],
      ts: 2,
    },
    {
      id: "a1",
      role: "assistant",
      content: [
        { type: "redacted_thinking", data: "opaque-blob" },
        { type: "text", text: "ok" },
      ],
      ts: 3,
    },
  ]);

  const [sealed, redacted] = Cline.visibleNodes(session).filter(
    (node) => node.role === "assistant",
  );
  expect(Option.getOrUndefined(sealed?.thinking ?? Option.none())).toBe("ponder");
  expect(Option.getOrUndefined(sealed?.thinkingSignature ?? Option.none())).toBe("sealed.v1.sig");
  // the redacted blob has no text — the marker stands in, `data` is the seal
  expect(Option.getOrUndefined(redacted?.thinking ?? Option.none())).toBe("[redacted]");
  expect(Option.getOrUndefined(redacted?.thinkingSignature ?? Option.none())).toBe("opaque-blob");

  // … and an export writes both back in provider shape.
  const messages = Cline.sessionMessages(session, "sealed").messages as ReadonlyArray<any>;
  const blocks = messages
    .filter((m) => m.role === "assistant")
    .flatMap((m) => m.content as ReadonlyArray<any>);
  expect(blocks).toContainEqual({
    type: "thinking",
    thinking: "ponder",
    signature: "sealed.v1.sig",
  });
  expect(blocks).toContainEqual({ type: "redacted_thinking", data: "opaque-blob" });
});

test("import keeps calls whose list fields arrived as strings", async () => {
  const session = await importedSession("stringy", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "call_1",
          name: "run_commands",
          input: { commands: '["pwd", "ls"]' },
        },
        { type: "tool_use", id: "call_2", name: "search_codebase", input: { queries: ":=" } },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: [
            { query: "pwd", result: "/work", success: true },
            { query: "ls", result: "a.ts", success: true },
          ],
        },
        {
          type: "tool_result",
          tool_use_id: "call_2",
          name: "search_codebase",
          content: [{ query: ":=", result: "x := 1", success: true }],
        },
      ],
      ts: 3,
    },
  ]);

  // a JSON-encoded list and a bare string both still describe the calls to make
  expect(toolResultContents(session)).toEqual(["/work", "a.ts", "x := 1"]);

  const messages = Cline.sessionMessages(session, "stringy").messages as ReadonlyArray<any>;
  expect(messages.map((message) => message.role)).toEqual(["user", "assistant", "user"]);
  expect(messages[1].content.map((block: any) => [block.name, block.input])).toEqual([
    ["run_commands", { commands: ["pwd"] }],
    ["run_commands", { commands: ["ls"] }],
    ["search_codebase", { queries: [":="] }],
  ]);
  expect(messages[2].content.map((block: any) => block.content)).toEqual([
    [{ query: "pwd", result: "/work", success: true }],
    [{ query: "ls", result: "a.ts", success: true }],
    [{ query: ":=", result: "x := 1", success: true }],
  ]);
});

test("import pairs each result entry with its call when the key differs", async () => {
  const session = await importedSession("ranges", [
    { id: "u0", role: "user", content: [{ type: "text", text: "read both" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "call_1",
          name: "read_files",
          input: { files: [{ path: "/work/a.ts" }, { path: "/work/b.ts" }] },
        },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "read_files",
          content: [
            { query: "/work/a.ts:1-40", result: "a body", success: true },
            { query: "/work/b.ts:5-9", result: "b body", success: true },
          ],
        },
      ],
      ts: 3,
    },
  ]);

  // the entries answer a line range the call never mentioned, yet each call
  // still keeps its own output instead of an empty result
  expect(toolResultContents(session)).toEqual(["a body", "b body"]);
});

test("import fills calls from result entries that carry no key at all", async () => {
  const session = await importedSession("unkeyed", [
    { id: "u0", role: "user", content: [{ type: "text", text: "run both" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "call_1",
          name: "run_commands",
          input: { commands: ["pwd", "ls"] },
        },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: [{ result: "first" }, { result: "second" }],
        },
      ],
      ts: 3,
    },
  ]);

  expect(toolResultContents(session)).toEqual(["first", "second"]);
});

test("import reads the shape variants a call can arrive in", async () => {
  const session = await importedSession("variants", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        { type: "tool_use", id: "call_1", name: "run_commands", input: { command: "pwd" } },
        {
          type: "tool_use",
          id: "call_2",
          name: "read_files",
          input: { path: "/work/a.ts", start_line: 10, end_line: 20 },
        },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: [{ query: "pwd", result: "/work", success: true }],
        },
        {
          type: "tool_result",
          tool_use_id: "call_2",
          name: "read_files",
          content: [{ query: "/work/a.ts:10-20", result: "a body", success: true }],
        },
      ],
      ts: 3,
    },
  ]);

  // a single `command` instead of `commands`, and a `path` with a line range
  expect(toolResultContents(session)).toEqual(["/work", "a body"]);
});

test("import keeps a call whose input carries nothing readable", async () => {
  const session = await importedSession("unreadable", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "call_1",
          name: "run_commands",
          input: { description: "check the build" },
        },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: "done",
        },
      ],
      ts: 3,
    },
  ]);

  // the call is kept as it arrived, so its output still has a caller
  expect(toolResultContents(session)).toEqual(["done"]);

  const messages = Cline.sessionMessages(session, "unreadable").messages as ReadonlyArray<any>;
  expect(messages[1].content.filter((block: any) => block.type === "tool_use")).toEqual([
    {
      type: "tool_use",
      id: expect.any(String),
      name: "run_commands",
      input: { description: "check the build" },
    },
  ]);
  expect(Cline.transcriptViolations(messages)).toEqual([]);
});

test("import pairs a result that arrived after a later assistant turn", async () => {
  const session = await importedSession("late", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        { type: "tool_use", id: "call_1", name: "run_commands", input: { commands: ["pwd"] } },
      ],
      ts: 2,
    },
    { id: "u1", role: "user", content: [{ type: "text", text: "continue" }], ts: 3 },
    { id: "a1", role: "assistant", content: [{ type: "text", text: "still working" }], ts: 4 },
    {
      id: "u2",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: [{ query: "pwd", result: "/work", success: true }],
        },
      ],
      ts: 5,
    },
  ]);

  // the call belongs to the first turn, so its output does too — it must not
  // decay into plain "[tool output]" text
  expect(toolResultContents(session)).toEqual(["/work"]);

  const messages = Cline.sessionMessages(session, "late").messages as ReadonlyArray<any>;
  expect(messages.map((message) => message.role)).toEqual([
    "user",
    "assistant",
    "user",
    "user",
    "assistant",
  ]);
  expect(messages[2].content[0]).toMatchObject({
    type: "tool_result",
    name: "run_commands",
    content: [{ query: "pwd", result: "/work", success: true }],
  });
  expect(Cline.transcriptViolations(messages)).toEqual([]);
});

test("import keeps answer parts that no call can hold", async () => {
  const session = await importedSession("excess", [
    { id: "u0", role: "user", content: [{ type: "text", text: "update the todos" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "call_1",
          name: "todo_write",
          input: { todos: [{ content: "a" }, { content: "b" }] },
        },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "todo_write",
          content: [
            { query: "todo 1", result: "updated 1", success: true },
            { query: "todo 2", result: "updated 2", success: true },
          ],
        },
      ],
      ts: 3,
    },
  ]);

  // the tool has no sub-calls to split into, so its whole answer must survive
  expect(toolResultContents(session)).toEqual(["updated 1\nupdated 2"]);
});

test("sessionMessages drops an assistant turn that carries nothing", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "user", content: "do the thing" }),
    makeNode({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "assistant",
      content: "",
      metadata: rendered,
    }),
    makeNode({ nodeId: 2, parentNodeId: Option.some(1), role: "user", content: "still there?" }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;

  // an empty turn is what the CLI's own import path drops, so sepia omits it too
  expect(messages.map((m) => m.role)).toEqual(["user", "user"]);
  expect(Cline.transcriptViolations(messages)).toEqual([]);
});

test("transcriptViolations reports what the CLI rejects when it resumes a log", () => {
  const call = {
    role: "assistant",
    content: [{ type: "tool_use", id: "call_1", name: "read_files", input: {} }],
  };
  const text = { role: "user", content: [{ type: "text", text: "continue" }] };
  const result = {
    role: "user",
    content: [{ type: "tool_result", tool_use_id: "call_1", name: "read_files", content: "body" }],
  };

  // a turn sandwiched between a call and its result is the reported failure …
  expect(Cline.transcriptViolations([call, text, result]).map((v) => v.index)).toEqual([1]);
  // … a result in front of it is fine, and a call left open at the end is not.
  expect(Cline.transcriptViolations([call, result, text])).toEqual([]);
  expect(Cline.transcriptViolations([call]).map((v) => v.index)).toEqual(["eof"]);
});

test("clineSessionId follows the CLI's <epoch-ms>_<5 alphanumerics> shape", () => {
  const id = Cline.clineSessionId(1789882145000);

  expect(id).toMatch(/^1789882145000_[a-z0-9]{5}$/);
});

test("sessionRow carries the whole index shape the Cline CLI reads", () => {
  const row = ClineIndex.sessionRow(
    makeSession(sampleNodes()),
    "1789882145000_sepia",
    "/home/luis/.cline/data/sessions/1789882145000_sepia/1789882145000_sepia.messages.json",
    "2026-09-20T05:29:05.000Z",
  );

  expect(Object.keys(row)).toEqual([...ClineIndex.SESSION_COLUMNS]);
  expect(row.session_id).toBe("1789882145000_sepia");
  expect(row.status).toBe("completed");
  expect(row.pid).toBe(0);
  expect(row.is_subagent).toBe(0);
  expect(row.prompt).toBe("do the thing");
  expect(row.cwd).toBe("/work");
  expect(row.workspace_root).toBe("/work");
  expect(row.transcript_path).toBe("");
  expect(row.hook_path).toBe("");

  const metadata = JSON.parse(row.metadata_json as string);
  expect(metadata.title).toBe("Imported session");
  expect(metadata.importedFrom).toEqual({ store: "devin", sessionId: "imported-session" });
  // placeholders and columns cannot drift apart.
  expect(ClineIndex.INSERT_SESSION.split("?").length - 1).toBe(ClineIndex.SESSION_COLUMNS.length);
});

test("isPidAlive treats dead, invalid, and own-process pids correctly", () => {
  expect(ClineIndex.isPidAlive(0)).toBe(false);
  expect(ClineIndex.isPidAlive(-1)).toBe(false);
  expect(ClineIndex.isPidAlive(2 ** 53)).toBe(false);
  expect(ClineIndex.isPidAlive(4_000_000_000)).toBe(false);
  expect(ClineIndex.isPidAlive(process.pid)).toBe(true);
});

test("isActiveRow matches abandoned and live-owned sessions correctly", () => {
  // a running session whose owner is gone is adoptable.
  expect(ClineIndex.isActiveRow({ status: "running", pid: 4_000_000_000 }, false)).toBe(false);
  // a dead run is replaceable even when the row keeps its stale pid.
  expect(ClineIndex.isActiveRow({ status: "completed", pid: process.pid }, true)).toBe(false);
  // an in-flight session with a signaling pid must not be replaced.
  expect(ClineIndex.isActiveRow({ status: "running", pid: process.pid }, true)).toBe(true);
  expect(ClineIndex.isActiveRow({ status: "idle", pid: process.pid }, true)).toBe(true);
  expect(ClineIndex.isActiveRow({ status: "pending", pid: process.pid }, true)).toBe(true);
});

test("import keeps token metrics and the per-message model", async () => {
  const session = await importedSession("metrics", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [{ type: "text", text: "done" }],
      ts: 2,
      modelInfo: { id: "deepseek/deepseek-v4-flash", provider: "cline-pass" },
      metrics: {
        inputTokens: 5720,
        outputTokens: 279,
        cacheReadTokens: 100,
        cacheWriteTokens: 4,
        cost: 0.02,
      },
    },
  ]);

  const assistant = session.nodes.find((n) => n.role === "assistant" && n.content === "done");
  expect(Option.getOrNull(assistant!.usage)).toEqual({
    input: 5720,
    output: 279,
    cacheRead: 100,
    cacheWrite: 4,
    cost: 0.02,
  });
  expect(Option.getOrNull(assistant!.model)).toBe("deepseek/deepseek-v4-flash");
});

test("import leaves usage/model empty for token-less or malformed message fields", async () => {
  const session = await importedSession("sparse", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    { id: "a0", role: "assistant", content: [{ type: "text", text: "a" }], ts: 2, metrics: "nope" },
    {
      id: "a1",
      role: "assistant",
      content: [{ type: "text", text: "b" }],
      ts: 3,
      metrics: { cacheReadTokens: 9 },
      modelInfo: { id: "", provider: "cline-pass" },
    },
    {
      id: "a2",
      role: "assistant",
      content: [{ type: "text", text: "c" }],
      ts: 4,
      metrics: { inputTokens: 1 },
    },
  ]);

  const text = (s: string) => session.nodes.find((n) => n.role === "assistant" && n.content === s)!;
  expect(Option.isNone(text("a").usage)).toBe(true);
  expect(Option.isNone(text("b").usage)).toBe(true);
  expect(Option.isNone(text("b").model)).toBe(true);
  expect(Option.getOrNull(text("c").usage)).toEqual({ input: 1, output: 0 });
});

test("import folds a failed result back onto the call and the result node", async () => {
  const session = await importedSession("failed", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    {
      id: "a0",
      role: "assistant",
      content: [
        { type: "tool_use", id: "call_1", name: "run_commands", input: { command: "exit 2" } },
      ],
      ts: 2,
    },
    {
      id: "u1",
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "call_1",
          name: "run_commands",
          content: [{ query: "exit 2", result: "", success: false }],
        },
      ],
      ts: 3,
    },
  ]);

  const tool = session.nodes.find((n) => n.role === "tool");
  expect(Option.getOrNull(tool!.toolResult)).toEqual({ status: "error" });

  // both assistant twins carry the outcome on their ToolCall
  const assistants = session.nodes.filter((n) => n.role === "assistant");
  expect(assistants.length).toBeGreaterThan(0);
  for (const a of assistants) {
    expect(a.toolCalls.map((tc) => Option.getOrNull(tc.status))).toEqual(["error"]);
  }
});

test("clineSubagentInfo reads lineage out of the session id", () => {
  expect(Cline.clineSubagentInfo("1788501677312_sh9yh")).toBeNull();
  expect(Cline.clineSubagentInfo("1788501677312_sh9yh__teamtask__astdata__hXujax")).toEqual({
    parentSessionId: "1788501677312_sh9yh",
    agentId: "astdata",
  });
  expect(Cline.clineSubagentInfo("1788512223880_qo9bf__agent_1788512292452_l58kf4")).toEqual({
    parentSessionId: "1788512223880_qo9bf",
    agentId: "agent_1788512292452_l58kf4",
  });
  // nested team tasks split at the last marker — the parent is one level up
  expect(
    Cline.clineSubagentInfo(
      "1789101927554_p6vq1__teamtask__subagent-env-removal__RBdWwq__teamtask__cranelift-env-removal__pUEZ9k",
    ),
  ).toEqual({
    parentSessionId: "1789101927554_p6vq1__teamtask__subagent-env-removal__RBdWwq",
    agentId: "cranelift-env-removal",
  });
  // a truncated marker yields no agent to name
  expect(Cline.clineSubagentInfo("x__teamtask__")).toBeNull();
});

test("import marks a sub-agent session's lineage from its id", async () => {
  const session = await importedSession("1788501677312_sh9yh__teamtask__astdata__hXujax", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
  ]);

  expect(Option.getOrNull(session.parentSessionId)).toBe("1788501677312_sh9yh");
  expect(Option.getOrNull(session.agentId)).toBe("astdata");
});

test("export writes usage, per-message model and result success back", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "user", content: "do the thing" }),
    MessageNode.make({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "assistant",
      content: "on it",
      toolCalls: [ToolCall.make({ id: "call_1", name: "read", arguments: { file_path: "/a" } })],
      usage: Option.some({ input: 10, output: 5, cacheRead: 7 }),
      model: Option.some("deepseek/deepseek-v4-flash"),
      createdAt: 1700000001,
      metadata: rendered,
    }),
    makeNode({
      nodeId: 2,
      parentNodeId: Option.some(1),
      role: "tool",
      content: "denied",
      toolCallId: Option.some("call_1"),
      toolName: Option.some("read"),
      toolResult: Option.some({ status: "error" }),
      metadata: { toolArguments: { file_path: "/a" } },
    }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;

  expect(messages[1].modelInfo).toEqual({
    id: "deepseek/deepseek-v4-flash",
    provider: "cline-pass",
  });
  expect(messages[1].metrics).toEqual({
    inputTokens: 10,
    outputTokens: 5,
    cacheReadTokens: 7,
    cacheWriteTokens: 0,
    cost: 0,
  });
  expect(messages[2].content[0].content).toEqual([
    { query: "/a", result: "denied", success: false },
  ]);
});

test("sessionRow records sub-agent lineage the index carries", () => {
  const session = Session.make({
    id: "1788501677312_sh9yh__teamtask__astdata__hXujax",
    title: "sub",
    workingDirectory: "/work",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: 0,
    parentSessionId: Option.some("1788501677312_sh9yh"),
    agentId: Option.some("astdata"),
    metadata: null,
    nodes: [],
  });
  const row = ClineIndex.sessionRow(session, session.id, "/tmp/m.messages.json");

  expect(row.parent_session_id).toBe("1788501677312_sh9yh");
  expect(row.agent_id).toBe("astdata");
  expect(row.is_subagent).toBe(1);
});

test("import lifts image and document blocks off user messages", async () => {
  const session = await importedSession("attachments", [
    {
      id: "u0",
      role: "user",
      content: [
        { type: "text", text: '<user_input mode="act">look at these</user_input>' },
        {
          type: "image",
          source: { type: "base64", media_type: "image/png", data: "aGk=" },
        },
        {
          type: "document",
          title: "spec.md",
          source: { type: "url", url: "file:///work/spec.md", media_type: "text/markdown" },
        },
      ],
      ts: 1,
    },
    {
      id: "u1",
      role: "user",
      content: [
        { type: "text", text: "and this" },
        { type: "image", source: { type: "url", url: "https://x/y.png" } },
        // Entries that carry nothing renderable are skipped, not fatal.
        { type: "image", source: { type: "base64" } },
        { type: "document", source: null },
        { type: "mystery", payload: 1 },
        "junk",
      ],
      ts: 2,
    },
    {
      // Flat field spellings — a block may carry data/url/media_type on
      // itself rather than under `source`.
      id: "u2",
      role: "user",
      content: [
        { type: "text", text: "flat fields" },
        { type: "image", data: "AAE=", media_type: "image/jpeg" },
        { type: "image", data: "AAF=", mimeType: "image/gif" },
        { type: "image", url: "https://flat/i.png" },
        { type: "document", url: "file:///b.md", text: "doc body" },
        { type: "document", source: { type: "base64", data: "AAI=" } },
        { type: "document", source: { type: "text", text: "src body" } },
        { type: "document", data: "AAQ=" },
      ],
      ts: 3,
    },
    {
      // A tool result rides a user-role message in this format — its content
      // stays a tool concern and never becomes a block.
      id: "u3",
      role: "user",
      content: [{ type: "tool_result", tool_use_id: "orphan", name: "exec", content: "x" }],
      ts: 4,
    },
  ]);

  const users = Cline.visibleNodes(session).filter((n) => n.role === "user");

  // The first user message seeds the tree — its attachments land there.
  expect(users[0]?.blocks).toEqual([
    { type: "text", text: '<user_input mode="act">look at these</user_input>' },
    { type: "image", data: "aGk=", mimeType: "image/png" },
    {
      type: "file",
      uri: "file:///work/spec.md",
      name: "spec.md",
      mimeType: "text/markdown",
    },
  ]);

  // Later user messages keep their own blocks.
  expect(users[1]?.blocks).toEqual([
    { type: "text", text: "and this" },
    { type: "image", uri: "https://x/y.png" },
  ]);

  // Flat spellings map the same way; a document's own text becomes the
  // file block's embedded text.
  expect(users[2]?.blocks).toEqual([
    { type: "text", text: "flat fields" },
    { type: "image", data: "AAE=", mimeType: "image/jpeg" },
    { type: "image", data: "AAF=", mimeType: "image/gif" },
    { type: "image", uri: "https://flat/i.png" },
    { type: "file", uri: "file:///b.md", text: "doc body" },
    { type: "file", data: "AAI=" },
    { type: "file", text: "src body" },
    { type: "file", data: "AAQ=" },
  ]);

  // The orphaned tool result decays to "[tool output]" text with no blocks.
  expect(users[3]?.content).toContain("[tool output]");
  expect(users[3]?.blocks).toEqual([]);
});

test("import survives a log whose first turn isn't a text-bearing user message", async () => {
  const session = await importedSession("nouser", [
    {
      id: "a0",
      role: "assistant",
      content: [{ type: "text", text: "booted mid-flight" }],
      ts: 1,
    },
    {
      id: "u0",
      role: "user",
      content: [
        { type: "image", source: { type: "url", url: "https://x/i.png" } },
        { type: "text" },
        { type: "text", text: "" },
        { type: "text", text: "now with text" },
      ],
      ts: 2,
    },
  ]);

  // The first text-bearing user message seeds the tree — non-text items
  // ahead of its text, an empty text block, and the image all land on the
  // seed node's blocks; the message is not emitted twice.
  const users = Cline.visibleNodes(session).filter((n) => n.role === "user");
  expect(users).toHaveLength(1);
  expect(users[0]?.content).toBe("now with text");
  expect(users[0]?.blocks).toEqual([
    { type: "image", uri: "https://x/i.png" },
    { type: "text", text: "" },
    { type: "text", text: "now with text" },
  ]);
});

test("import tolerates empty, missing and mixed user content", async () => {
  const session = await importedSession("mixed", [
    // No content field at all.
    { id: "u_pre", role: "user", ts: 0 },
    // A text entry with no text, mixed with a tool result: the text half is
    // dropped (a tool-result message is not a prompt) and the orphan decays.
    {
      id: "u0",
      role: "user",
      content: [
        { type: "text" },
        { type: "tool_result", tool_use_id: "or", name: "exec", content: "y" },
        // A result entry may even lack the call id entirely.
        { type: "tool_result", name: "exec", content: "z" },
      ],
      ts: 1,
    },
    { id: "u1", role: "user", content: [{ type: "text", text: "go" }], ts: 2 },
    // A prompt turn whose only text entry carries no text emits nothing.
    { id: "u2", role: "user", content: [{ type: "text" }], ts: 3 },
  ]);

  // The "go" turn seeds the tree; the orphaned results decay to tool-output
  // text; the empty/missing contents produce nothing.
  const users = session.nodes.filter((n) => n.role === "user");
  expect(users.map((n) => n.content)).toEqual(["go", "[tool output]\ny", "[tool output]\nz"]);
  expect(users.every((n) => n.blocks.length === 0)).toBe(true);
});

test("import builds a session from a log with no user text at all", async () => {
  const session = await importedSession("assistantonly", [
    {
      id: "a0",
      role: "assistant",
      content: [{ type: "text", text: "spoke unprompted" }],
      ts: 1,
    },
  ]);

  // The seed user node stays empty — nothing to lift text or blocks from.
  const users = session.nodes.filter((n) => n.role === "user");
  expect(users).toHaveLength(1);
  expect(users[0]?.content).toBe("");
  expect(users[0]?.blocks).toEqual([]);
});

test("import keeps a text-only content array off node.blocks", async () => {
  const session = await importedSession("textonly", [
    { id: "u0", role: "user", content: [{ type: "text", text: "go" }], ts: 1 },
    { id: "u1", role: "user", content: [{ type: "text", text: "more" }], ts: 2 },
    { id: "u2", role: "user", content: "bare string", ts: 3 },
  ]);

  for (const node of session.nodes) {
    expect(node.blocks).toEqual([]);
  }
});

test("export writes user blocks back as provider content entries", () => {
  const session = makeSession([
    makeNode({ nodeId: 0, role: "system", content: "sys" }),
    MessageNode.make({
      nodeId: 1,
      role: "user",
      content: "with attachments",
      blocks: [
        { type: "text", text: "with attachments" },
        { type: "image", data: "aGk=", mimeType: "image/png" },
        { type: "image", data: "aGk=" },
        { type: "image", uri: "https://x/y.png" },
        { type: "image" },
        { type: "audio", data: "AAE=", mimeType: "audio/wav" },
        { type: "audio" },
        { type: "file", name: "spec.md", uri: "file:///work/spec.md" },
        { type: "file", uri: "file:///a.ts", text: "const a=1" },
        { type: "file" },
      ],
      createdAt: 1700000000,
      metadata: null,
    }),
  ]);

  const messages = Cline.sessionMessages(session, "imported-session")
    .messages as ReadonlyArray<any>;

  // Text stays a single block from `content`; each attachment maps to the
  // provider form (embedded image) or degrades to a mention.
  expect(messages[0].content).toEqual([
    { type: "text", text: "with attachments" },
    { type: "image", source: { type: "base64", media_type: "image/png", data: "aGk=" } },
    { type: "image", source: { type: "base64", data: "aGk=" } },
    { type: "image", source: { type: "url", url: "https://x/y.png" } },
    { type: "text", text: "[image]" },
    { type: "text", text: "[audio: audio/wav]" },
    { type: "text", text: "[audio: attachment]" },
    { type: "text", text: "[file: spec.md]" },
    { type: "text", text: "[file: file:///a.ts]\nconst a=1" },
    { type: "text", text: "[file: attachment]" },
  ]);
});

test("needsMigration only fires when a required table is missing", () => {
  expect(needsMigration(new Set(["sessions", "message_nodes", "prompt_history"]))).toBe(false);
  expect(
    needsMigration(
      new Set(["sessions", "message_nodes", "prompt_history", "__drizzle_migrations"]),
    ),
  ).toBe(false);
  expect(needsMigration(new Set())).toBe(true);
  expect(needsMigration(new Set(["sessions", "message_nodes"]))).toBe(true);
});
