/**
 * Writer-side coverage for `toJsonl`/`blockToClaude` plus reader edge
 * branches `ClaudeCode.test.ts` does not reach (one-sided usage fields,
 * non-text tool_result content, dropped content items).
 */
import { Option } from "effect";
import { expect, test } from "vite-plus/test";
import * as ClaudeCode from "../src/ClaudeCode.js";
import { MessageNode, REDACTED_THINKING, Session, ToolCall } from "sepia-core";

const line = (entry: Record<string, unknown>): string => JSON.stringify(entry);

const userEntry = (uuid: string, parentUuid: string | null, content: unknown, extra = {}) => ({
  type: "user",
  uuid,
  parentUuid,
  sessionId: "sess-1",
  isSidechain: false,
  cwd: "/work/proj",
  timestamp: "2026-01-01T00:00:00.000Z",
  message: { role: "user", content },
  ...extra,
});

const node = (over: Partial<Parameters<typeof MessageNode.make>[0]>): MessageNode =>
  MessageNode.make({
    nodeId: 0,
    role: "user",
    content: "hello",
    createdAt: 1_700_000_000,
    metadata: null,
    ...over,
  });

const session = (nodes: ReadonlyArray<MessageNode>, over = {}): Session =>
  Session.make({
    id: "sess-1",
    title: "Title",
    workingDirectory: "/work/proj",
    model: "session-model",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_010,
    mainChainId: nodes.length - 1,
    metadata: {},
    nodes,
    ...over,
  });

const entriesOf = (jsonl: string): ReadonlyArray<Record<string, unknown>> =>
  jsonl
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as Record<string, unknown>);

test("toJsonl writes every attachment block form and drops unmappable ones", () => {
  const writer = session([
    node({
      nodeId: 0,
      content: "see attachments",
      blocks: [
        { type: "text", text: "see attachments" },
        { type: "image", data: "AAA=", mimeType: "image/jpeg" },
        { type: "image", data: "BBB=" },
        { type: "image", uri: "https://x/img.png" },
        { type: "image" },
        { type: "file", text: "doc body", name: "notes.txt" },
        { type: "file", text: "md", mimeType: "text/markdown" },
        { type: "file", data: "RA==", name: "blob.bin" },
        { type: "file", data: "Ug==" },
        { type: "file", uri: "https://x/doc.pdf", name: "doc.pdf" },
        { type: "file" },
        { type: "audio", data: "QQ==" },
      ],
    }),
  ]);

  const [entry] = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "user");
  const content = (entry!.message as { content: ReadonlyArray<unknown> }).content;
  expect(content).toEqual([
    { type: "text", text: "see attachments" },
    {
      type: "image",
      source: { type: "base64", media_type: "image/jpeg", data: "AAA=" },
    },
    { type: "image", source: { type: "base64", media_type: "image/png", data: "BBB=" } },
    { type: "image", source: { type: "url", url: "https://x/img.png" } },
    {
      type: "document",
      source: { type: "text", media_type: "text/plain", text: "doc body" },
      title: "notes.txt",
    },
    { type: "document", source: { type: "text", media_type: "text/markdown", text: "md" } },
    {
      type: "document",
      source: { type: "base64", media_type: "application/octet-stream", data: "RA==" },
      title: "blob.bin",
    },
    {
      type: "document",
      source: { type: "base64", media_type: "application/octet-stream", data: "Ug==" },
    },
    { type: "document", source: { type: "url", url: "https://x/doc.pdf" }, title: "doc.pdf" },
  ]);
  // the bare image, bare file and audio blocks emitted nothing
  expect(content).toHaveLength(9);
});

test("toJsonl falls back to node.content when blocks are empty and keeps flag metadata", () => {
  const writer = session([
    node({
      nodeId: 0,
      content: "meta entry",
      metadata: {
        uuid: "u-flagged",
        isMeta: true,
        isCompactSummary: true,
        isVisibleInTranscriptOnly: true,
      },
    }),
  ]);
  const [entry] = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "user");
  expect(entry!.isMeta).toBe(true);
  expect(entry!.isCompactSummary).toBe(true);
  expect(entry!.isVisibleInTranscriptOnly).toBe(true);
  expect(entry!.uuid).toBe("u-flagged");
  expect((entry!.message as { content: unknown }).content).toBe("meta entry");
});

test("toJsonl mints uuids for nodes without one and emits parent links", () => {
  const writer = session([
    node({ nodeId: 0, content: "first", metadata: null }),
    node({
      nodeId: 1,
      parentNodeId: Option.some(0),
      content: "second",
      metadata: 42 as unknown,
    }),
    // a parent index that resolves to no emitted uuid stays null
    node({ nodeId: 2, parentNodeId: Option.some(99), content: "orphan" }),
  ]);
  const entries = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "user");
  expect(entries[0]!.uuid).toBeTruthy();
  expect(entries[1]!.uuid).toBeTruthy();
  expect(entries[1]!.uuid).not.toBe(entries[0]!.uuid);
  expect(entries[1]!.parentUuid).toBe(entries[0]!.uuid);
  expect(entries[2]!.parentUuid).toBeNull();
});

test("toJsonl writes tool nodes with the recorded toolUseResult and empty id fallback", () => {
  const writer = session([
    node({ nodeId: 0, content: "run" }),
    node({
      nodeId: 1,
      parentNodeId: Option.some(0),
      role: "tool",
      content: "output",
      toolCallId: Option.some("toolu_1"),
      toolResult: Option.some({ status: "error" }),
      metadata: { toolUseResult: { status: "completed" } },
    }),
    node({
      nodeId: 2,
      role: "tool",
      content: "no id",
      // no toolCallId — the entry still needs the field
    }),
  ]);
  const entries = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "user");
  const toolEntry = entries[1]!;
  expect(toolEntry.toolUseResult).toEqual({ status: "completed" });
  const content = (toolEntry.message as { content: ReadonlyArray<Record<string, unknown>> })
    .content;
  expect(content[0]).toMatchObject({
    type: "tool_result",
    tool_use_id: "toolu_1",
    content: "output",
    is_error: true,
  });
  const bare = (entries[2]!.message as { content: ReadonlyArray<Record<string, unknown>> })
    .content[0]!;
  expect(bare.tool_use_id).toBe("");
  expect(bare.is_error).toBe(false);
});

test("toJsonl assistant entries carry model/request/usage fallbacks", () => {
  const writer = session(
    [
      node({ nodeId: 0, content: "go" }),
      node({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "assistant",
        content: "",
        // a sealed redacted block with an empty text body
        thinking: Option.some(REDACTED_THINKING),
        thinkingSignature: Option.some("blob"),
        toolCalls: [ToolCall.make({ id: "c1", name: "Bash", arguments: {} })],
        requestId: Option.some("req_9"),
        metadata: { messageId: "msg_recorded" },
        usage: Option.some({ input: 3, output: 1 }),
      }),
      node({
        nodeId: 2,
        role: "assistant",
        content: "plain",
        model: Option.some("node-model"),
        finishReason: Option.some("max_tokens"),
        usage: Option.some({ input: 1, output: 2, cacheRead: 5, cacheWrite: 7 }),
      }),
    ],
    { metadata: { gitBranch: "dev", claudeVersion: "2.2.0", slug: "sluggy" } },
  );

  const assistants = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "assistant");
  const first = assistants[0]!;
  // no text body, no recorded signature text — the redacted blob replays
  expect(first.message).toMatchObject({
    id: "msg_recorded",
    model: "session-model", // node has no model → the session's
    stop_reason: "tool_use", // unresolved calls imply a tool_use stop
    usage: { input_tokens: 3, output_tokens: 1 },
  });
  expect((first.message as { content: ReadonlyArray<unknown> }).content).toEqual([
    { type: "redacted_thinking", data: "blob" },
    { type: "tool_use", id: "c1", name: "Bash", input: {} },
  ]);
  expect(first.requestId).toBe("req_9");
  expect(first.gitBranch).toBe("dev");
  expect(first.version).toBe("2.2.0");
  expect(first.slug).toBe("sluggy");

  const second = assistants[1]!;
  expect(second.message).toMatchObject({
    model: "node-model",
    stop_reason: "max_tokens",
    usage: {
      input_tokens: 1,
      output_tokens: 2,
      cache_read_input_tokens: 5,
      cache_creation_input_tokens: 7,
    },
  });
  // no recorded messageId — one is minted from the entry uuid
  expect(String((second.message as { id: string }).id)).toMatch(/^msg_/);
  expect(second.requestId).toBeUndefined();
});

test("toJsonl writes system nodes with subtype/level/compactMetadata", () => {
  const writer = session([
    node({
      nodeId: 0,
      role: "system",
      content: "init banner",
      metadata: { subtype: "init", level: "info", compactMetadata: { preTokens: 10 } },
    }),
    node({
      nodeId: 1,
      role: "system",
      content: "bare",
      metadata: null,
    }),
  ]);
  const systems = entriesOf(ClaudeCode.toJsonl(writer)).filter((e) => e.type === "system");
  expect(systems[0]).toMatchObject({
    subtype: "init",
    content: "init banner",
    level: "info",
    compactMetadata: { preTokens: 10 },
  });
  // no recorded subtype — the entry defaults to "init" with no extras
  expect(systems[1]).toMatchObject({ subtype: "init", content: "bare" });
  expect(systems[1]!.level).toBeUndefined();
  expect(systems[1]!.compactMetadata).toBeUndefined();
});

/* ---- reader edge branches ------------------------------------------ */

test("assistant content items of an unknown type are skipped", () => {
  const parsed = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line({
        type: "assistant",
        uuid: "u2",
        parentUuid: "u1",
        timestamp: "2026-01-01T00:00:01.000Z",
        message: {
          role: "assistant",
          content: [
            { type: "image", source: { type: "url", url: "https://x/i.png" } },
            { type: "text", text: "answer" },
          ],
        },
      }),
    ].join("\n"),
    { id: "s" },
  );
  expect(parsed.nodes[1].content).toBe("answer");
});

test("usage with one-sided counts and partial cache tiers defaults the rest", () => {
  const parsed = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line({
        type: "assistant",
        uuid: "u2",
        parentUuid: "u1",
        timestamp: "2026-01-01T00:00:01.000Z",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "a" }],
          usage: { input_tokens: 4 },
        },
      }),
      line({
        type: "assistant",
        uuid: "u3",
        parentUuid: "u2",
        timestamp: "2026-01-01T00:00:02.000Z",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "b" }],
          usage: {
            output_tokens: 9,
            cache_creation: { ephemeral_5m_input_tokens: 3 },
          },
        },
      }),
      line({
        type: "assistant",
        uuid: "u4",
        parentUuid: "u3",
        timestamp: "2026-01-01T00:00:03.000Z",
        message: {
          role: "assistant",
          content: [{ type: "text", text: "c" }],
          usage: {
            output_tokens: 2,
            cache_creation: { ephemeral_1h_input_tokens: 6 },
          },
        },
      }),
    ].join("\n"),
    { id: "s" },
  );
  expect(Option.getOrUndefined(parsed.nodes[1].usage)).toEqual({ input: 4, output: 0 });
  expect(Option.getOrUndefined(parsed.nodes[2].usage)).toEqual({
    input: 0,
    output: 9,
    cacheWrite: 3,
  });
  expect(Option.getOrUndefined(parsed.nodes[3].usage)).toEqual({
    input: 0,
    output: 2,
    cacheWrite: 6,
  });
});

test("a tool_result without content yields empty text; text blocks without text drop", () => {
  const parsed = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line(
        userEntry("u2", "u1", [
          { type: "tool_result", tool_use_id: "t1" },
          { type: "text" },
          { type: "text", text: "kept" },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const tool = parsed.nodes.find((n) => n.role === "tool");
  expect(tool).toBeDefined();
  expect(tool!.content).toBe("");
  // the text block with no text field folded away; only "kept" survives on
  // the user node the same entry produced
  const userNodes = parsed.nodes.filter((n) => n.role === "user");
  expect(userNodes.map((n) => n.content)).toContain("kept");
});

/* ---- repository defensive fallbacks ---------------------------------- */

import { mkdtempSync, mkdirSync, symlinkSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Effect } from "effect";
import * as ClaudeCodeRepository from "../src/ClaudeCodeRepository.js";

const makeStore = (files: Record<string, string>): string => {
  const root = mkdtempSync(join(tmpdir(), "sepia-claude-edge-"));
  for (const [rel, content] of Object.entries(files)) {
    const filePath = join(root, rel);
    mkdirSync(dirname(filePath), { recursive: true });
    writeFileSync(filePath, content);
  }
  return root;
};

const transcript = line(userEntry("u1", null, "hello"));

const baseSession = (over: Partial<Parameters<typeof Session.make>[0]> = {}): Session =>
  Session.make({
    id: "sess-2",
    title: "t",
    workingDirectory: "/work/proj",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: 0,
    metadata: null,
    nodes: [node({ nodeId: 0, content: "hi" })],
    ...over,
  });

test("list treats an unstatable projects root as empty", async () => {
  // A path no stat can resolve (embedded NUL) fails `exists` outright —
  // the repository degrades to an empty listing rather than throwing.
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({
    projectsDir: "sepia-\0-invalid",
  });
  expect(await Effect.runPromise(repo.list())).toEqual([]);
});

test("delete rejects an unsafe id and ignores unknown ones", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": transcript });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });

  await expect(Effect.runPromise(repo.delete("a/b"))).rejects.toThrow("not a safe file name");
  // unknown id: the scan finds nothing to remove and resolves quietly
  await Effect.runPromise(repo.delete("ghost"));
  expect(existsSync(join(projectsDir, "-work-proj/sess-1.jsonl"))).toBe(true);
});

test("save places a subagent under the project dir that already holds its parent", async () => {
  // The parent's transcript lives under a slug that does NOT match the
  // subagent's own cwd — save must reuse the holder's dir anyway.
  const projectsDir = makeStore({ "-elsewhere/parent-1.jsonl": transcript });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  const child = baseSession({
    id: "agent-7",
    workingDirectory: "/work/proj",
    parentSessionId: Option.some("parent-1"),
  });
  await Effect.runPromise(repo.save(child));
  expect(existsSync(join(projectsDir, "-elsewhere/parent-1/subagents/agent-7.jsonl"))).toBe(true);
});

test("save tolerates a projects root it cannot list and a broken parent probe", async () => {
  // projectsDir is a file: readDirectory fails and the holder scan degrades
  // to "no known holder" before the (also failing) mkdir surfaces.
  const root = mkdtempSync(join(tmpdir(), "sepia-claude-root-"));
  const notADir = join(root, "projects");
  writeFileSync(notADir, "nope");
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir: notADir });
  const child = baseSession({ id: "agent-7", parentSessionId: Option.some("parent-1") });
  await expect(Effect.runPromise(repo.save(child))).rejects.toThrow(
    "Failed to save claude session",
  );

  // A candidate project holding a symlink-loop where the parent transcript
  // would sit: the exists() probe fails (ELOOP) and the scan moves on.
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": transcript });
  mkdirSync(join(projectsDir, "-other"), { recursive: true });
  const loop = join(projectsDir, "-other/parent-9.jsonl");
  symlinkSync(loop, loop);
  const repo2 = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  await Effect.runPromise(
    repo2.save(
      baseSession({
        id: "agent-9",
        workingDirectory: "/work/proj",
        parentSessionId: Option.some("parent-9"),
      }),
    ),
  );
  expect(existsSync(join(projectsDir, "-work-proj/parent-9/subagents/agent-9.jsonl"))).toBe(true);
});

test("delete survives an unresolvable side-dir probe", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-5.jsonl": transcript });
  // `<slug>/sess-5` is a symlink loop — the post-delete exists() check fails
  // with ELOOP instead of answering, and delete still completes.
  const loop = join(projectsDir, "-work-proj/sess-5");
  symlinkSync(loop, loop);
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  await Effect.runPromise(repo.delete("sess-5"));
  expect(existsSync(join(projectsDir, "-work-proj/sess-5.jsonl"))).toBe(false);
});
