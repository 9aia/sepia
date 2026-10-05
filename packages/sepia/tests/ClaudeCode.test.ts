import * as Path from "@effect/platform/Path";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import { Effect, Layer, Option } from "effect";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { expect, test } from "vite-plus/test";
import * as ClaudeCode from "../src/ClaudeCode.js";
import * as ClaudeCodeRepository from "../src/ClaudeCodeRepository.js";
import { Session } from "../src/Domain.js";

const line = (entry: Record<string, unknown>): string => JSON.stringify(entry);

const userEntry = (uuid: string, parentUuid: string | null, content: unknown, extra = {}) => ({
  type: "user",
  uuid,
  parentUuid,
  sessionId: "sess-1",
  isSidechain: false,
  cwd: "/work/proj",
  gitBranch: "main",
  version: "2.1.0",
  timestamp: "2026-01-01T00:00:00.000Z",
  message: { role: "user", content },
  ...extra,
});

const assistantEntry = (
  uuid: string,
  parentUuid: string,
  content: ReadonlyArray<unknown>,
  extra: Record<string, unknown> = {},
  entry: Record<string, unknown> = {},
) => ({
  type: "assistant",
  uuid,
  parentUuid,
  sessionId: "sess-1",
  isSidechain: false,
  requestId: "req_1",
  timestamp: "2026-01-01T00:00:01.000Z",
  message: {
    id: "msg_1",
    role: "assistant",
    model: "claude-opus-4-5",
    content,
    stop_reason: "end_turn",
    usage: {
      input_tokens: 100,
      output_tokens: 40,
      cache_read_input_tokens: 12,
      cache_creation_input_tokens: 8,
    },
    ...extra,
  },
  ...entry,
});

test("fromJsonl maps entries to a linked node tree", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({ type: "summary", summary: "Fix the login bug", leafUuid: "u3" }),
      line(userEntry("u1", null, "fix the login bug please")),
      line(
        assistantEntry("u2", "u1", [
          { type: "thinking", thinking: "look at auth", signature: "sig-1" },
          { type: "text", text: "I'll check the auth module" },
          { type: "tool_use", id: "toolu_1", name: "Read", input: { file_path: "/src/a.ts" } },
        ]),
      ),
      line(
        userEntry("u3", "u2", [
          {
            type: "tool_result",
            tool_use_id: "toolu_1",
            content: "file contents",
            is_error: false,
          },
        ]),
      ),
      line(
        assistantEntry("u4", "u3", [{ type: "text", text: "done" }], {
          usage: { input_tokens: 5, output_tokens: 2 },
        }),
      ),
    ].join("\n"),
    { id: "sess-1" },
  );

  expect(session.id).toBe("sess-1");
  expect(session.title).toBe("Fix the login bug");
  expect(session.workingDirectory).toBe("/work/proj");
  expect(session.backendType).toBe("claude");
  expect(session.agentMode).toBe("accept-edits");
  expect(session.model).toBe("claude-opus-4-5");
  expect(session.createdAt).toBe(Math.floor(new Date("2026-01-01T00:00:00Z").getTime() / 1000));
  expect(session.lastActivityAt).toBe(
    Math.floor(new Date("2026-01-01T00:00:01Z").getTime() / 1000),
  );
  expect(session.mainChainId).toBe(session.nodes.length - 1);
  expect(Option.isNone(session.parentSessionId)).toBe(true);
  expect((session.metadata as any).gitBranch).toBe("main");
  expect((session.metadata as any).claudeVersion).toBe("2.1.0");
  expect((session.metadata as any).source).toBe("claude-code");

  const [user, assistant, tool, last] = session.nodes;
  expect(user.role).toBe("user");
  expect(Option.isNone(user.parentNodeId)).toBe(true);
  expect(user.content).toBe("fix the login bug please");

  expect(assistant.role).toBe("assistant");
  expect(Option.getOrUndefined(assistant.parentNodeId)).toBe(user.nodeId);
  expect(Option.getOrUndefined(assistant.thinking)).toBe("look at auth");
  expect(Option.getOrUndefined(assistant.thinkingSignature)).toBe("sig-1");
  expect(assistant.toolCalls).toHaveLength(1);
  expect(assistant.toolCalls[0].name).toBe("Read");
  expect(assistant.toolCalls[0].arguments).toEqual({ file_path: "/src/a.ts" });
  // the tool result's success is folded back onto the call
  expect(Option.getOrUndefined(assistant.toolCalls[0].status)).toBe("success");
  expect(Option.getOrUndefined(assistant.requestId)).toBe("req_1");
  expect(Option.getOrUndefined(assistant.finishReason)).toBe("end_turn");
  expect(Option.getOrUndefined(assistant.model)).toBe("claude-opus-4-5");
  expect(Option.getOrUndefined(assistant.usage)).toEqual({
    input: 100,
    output: 40,
    cacheRead: 12,
    cacheWrite: 8,
  });

  expect(tool.role).toBe("tool");
  expect(Option.getOrUndefined(tool.parentNodeId)).toBe(assistant.nodeId);
  expect(Option.getOrUndefined(tool.toolCallId)).toBe("toolu_1");
  expect(Option.getOrUndefined(tool.toolName)).toBe("Read");
  expect(Option.getOrUndefined(tool.toolResult)?.status).toBe("success");
  expect(tool.content).toBe("file contents");
  expect((tool.metadata as any).toolArguments).toEqual({ file_path: "/src/a.ts" });

  expect(last.role).toBe("assistant");
  expect(Option.getOrUndefined(last.usage)).toEqual({ input: 5, output: 2 });

  // prompt history is the user's own text, not tool results
  expect(session.promptHistory.map((p) => p.content)).toEqual(["fix the login bug please"]);
});

test("usage decodes nested ephemeral cache_creation tiers", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line(
        assistantEntry("u2", "u1", [{ type: "text", text: "hey" }], {
          usage: {
            input_tokens: 10,
            output_tokens: 3,
            cache_creation: {
              ephemeral_5m_input_tokens: 7,
              ephemeral_1h_input_tokens: 11,
            },
          },
        }),
      ),
    ].join("\n"),
    { id: "s" },
  );
  expect(Option.getOrUndefined(session.nodes[1].usage)).toEqual({
    input: 10,
    output: 3,
    cacheWrite: 18,
  });
});

test("user block arrays become nodes with attachments in blocks", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(
        userEntry("u1", null, [
          { type: "text", text: "see this" },
          {
            type: "image",
            source: { type: "base64", media_type: "image/png", data: "AAA=" },
          },
          {
            type: "document",
            source: { type: "text", media_type: "text/plain", text: "doc body" },
            title: "notes.txt",
          },
          { type: "image", source: { type: "url", url: "https://x/img.png" } },
          { type: "image" },
          { type: "image", data: "BBB", mimeType: "image/jpeg" },
          { type: "image", url: "https://x/direct.png", media_type: "image/gif" },
          { type: "document", url: "https://x/doc.pdf", media_type: "application/pdf" },
          { type: "document", data: "RA==" },
          { type: "document" },
          { type: "unknown-block", payload: 1 },
          "not-an-object",
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const node = session.nodes[0];
  expect(node.role).toBe("user");
  expect(node.content).toBe("see this");
  expect(node.blocks).toEqual([
    { type: "text", text: "see this" },
    { type: "image", data: "AAA=", mimeType: "image/png" },
    { type: "file", text: "doc body", name: "notes.txt", mimeType: "text/plain" },
    { type: "image", uri: "https://x/img.png" },
    { type: "image", data: "BBB", mimeType: "image/jpeg" },
    { type: "image", uri: "https://x/direct.png", mimeType: "image/gif" },
    { type: "file", uri: "https://x/doc.pdf", mimeType: "application/pdf" },
    { type: "file", data: "RA==" },
  ]);
});

test("all-text content arrays leave blocks empty", () => {
  const session = ClaudeCode.fromJsonl(
    line(
      userEntry("u1", null, [
        { type: "text", text: "a" },
        { type: "text", text: "b" },
      ]),
    ),
    { id: "s" },
  );
  expect(session.nodes[0].content).toBe("a\nb");
  expect(session.nodes[0].blocks).toEqual([]);
});

test("errored tool results mark the call and the tool node as error", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "run it")),
      line(
        assistantEntry("u2", "u1", [
          { type: "tool_use", id: "toolu_9", name: "Bash", input: { command: "false" } },
        ]),
      ),
      line(
        userEntry("u3", "u2", [
          {
            type: "tool_result",
            tool_use_id: "toolu_9",
            content: [{ type: "text", text: "exit 1" }],
            is_error: true,
          },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const tool = session.nodes[2];
  expect(Option.getOrUndefined(tool.toolResult)?.status).toBe("error");
  expect(tool.content).toBe("exit 1");
  expect(Option.getOrUndefined(session.nodes[1].toolCalls[0].status)).toBe("error");
});

test("orphaned tool results keep their output without a resolved tool name", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line(
        userEntry("u2", "u1", [
          { type: "tool_result", tool_use_id: "toolu_gone", content: "late output" },
          { type: "tool_result", content: 42 },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const [orphan, bare] = session.nodes.slice(1);
  expect(orphan.role).toBe("tool");
  expect(Option.getOrUndefined(orphan.toolCallId)).toBe("toolu_gone");
  expect(Option.isNone(orphan.toolName)).toBe(true);
  expect((orphan.metadata as any).toolArguments).toBeNull();
  expect(bare.role).toBe("tool");
  expect(Option.isNone(bare.toolCallId)).toBe(true);
  expect(bare.content).toBe("42");
});

test("non-text tool_result content serializes; text joins", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line(
        userEntry("u2", "u1", [
          {
            type: "tool_result",
            tool_use_id: "t1",
            content: [
              { type: "text", text: "first" },
              { type: "text", text: "second" },
            ],
          },
          {
            type: "tool_result",
            tool_use_id: "t2",
            content: [{ type: "image", source: { type: "base64", data: "AA" } }],
          },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  expect(session.nodes[1].content).toBe("first\nsecond");
  expect(session.nodes[2].content).toContain("image");
});

test("system entries become system nodes with subtype metadata", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({
        type: "system",
        subtype: "init",
        uuid: "s0",
        parentUuid: null,
        sessionId: "sess-1",
        timestamp: "2026-01-01T00:00:00.000Z",
      }),
      line(userEntry("u1", "s0", "hello")),
      line({
        type: "system",
        subtype: "compact_boundary",
        content: "Conversation compacted",
        level: "info",
        compactMetadata: { trigger: "auto", preTokens: 1000 },
        uuid: "s1",
        parentUuid: "u1",
        timestamp: "2026-01-01T00:00:02.000Z",
      }),
    ].join("\n"),
    { id: "s" },
  );
  const [init, , compact] = session.nodes;
  expect(init.role).toBe("system");
  expect(init.content).toBe("[claude init]");
  expect((init.metadata as any).subtype).toBe("init");
  expect(compact.role).toBe("system");
  expect(compact.content).toBe("Conversation compacted");
  expect((compact.metadata as any).compactMetadata).toEqual({ trigger: "auto", preTokens: 1000 });
});

test("summary-free files title from the first user message", () => {
  const session = ClaudeCode.fromJsonl(line(userEntry("u1", null, `  ${"word ".repeat(30)}  `)), {
    id: "untitled-id",
  });
  expect(session.title).toBe("word ".repeat(30).trim().slice(0, 80));
  expect(session.title.length).toBeLessThanOrEqual(80);

  const empty = ClaudeCode.fromJsonl("", { id: "bare" });
  expect(empty.title).toBe("bare");
  expect(empty.nodes).toEqual([]);
  expect(empty.workingDirectory).toBe("/");
});

test("fallbackCwd covers entries without cwd; last gitBranch wins", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({ type: "user", uuid: "u1", parentUuid: null, message: { content: "hi" } }),
      line({
        type: "user",
        uuid: "u2",
        parentUuid: "u1",
        gitBranch: "feature/x",
        message: { content: "again" },
      }),
    ].join("\n"),
    { id: "s", fallbackCwd: ClaudeCode.decodeProjectDir("-home-luis-proj") },
  );
  expect(session.workingDirectory).toBe("/home/luis/proj");
  expect((session.metadata as any).gitBranch).toBe("feature/x");
});

test("subagent files map isSidechain + sessionId to parentSessionId and agentId", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({
        type: "user",
        uuid: "a1",
        parentUuid: null,
        sessionId: "parent-uuid",
        agentId: "agent-42",
        isSidechain: true,
        cwd: "/work/proj",
        timestamp: "2026-01-01T00:00:00.000Z",
        message: { role: "user", content: "subtask prompt" },
      }),
      line({
        type: "assistant",
        uuid: "a2",
        parentUuid: "a1",
        sessionId: "parent-uuid",
        agentId: "agent-42",
        isSidechain: true,
        timestamp: "2026-01-01T00:00:01.000Z",
        message: {
          role: "assistant",
          model: "claude-haiku-4-5",
          content: [{ type: "text", text: "working" }],
        },
      }),
    ].join("\n"),
    { id: "agent-42" },
  );
  expect(Option.getOrUndefined(session.parentSessionId)).toBe("parent-uuid");
  expect(Option.getOrUndefined(session.agentId)).toBe("agent-42");
  expect((session.nodes[1].metadata as any).isSidechain).toBe(true);
  expect(session.model).toBe("claude-haiku-4-5");
});

test("agent-* files derive agentId from the filename when entries omit it", () => {
  const session = ClaudeCode.fromJsonl(
    line({
      type: "user",
      uuid: "a1",
      parentUuid: null,
      sessionId: "parent-uuid",
      isSidechain: true,
      message: { role: "user", content: "task" },
    }),
    { id: "agent-77", parentSessionId: "parent-uuid" },
  );
  expect(Option.getOrUndefined(session.agentId)).toBe("77");
  expect(Option.getOrUndefined(session.parentSessionId)).toBe("parent-uuid");
});

test("inline sidechain roots form a separate tree in the same session", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "main question")),
      line(userEntry("u2", "u1", "more")),
      line(userEntry("s1", null, "sidechain root", { isSidechain: true })),
    ].join("\n"),
    { id: "sess-1" },
  );
  expect(Option.isNone(session.nodes[2].parentNodeId)).toBe(true);
  // sessionId matches the file id, so no parentSessionId is inferred
  expect(Option.isNone(session.parentSessionId)).toBe(true);
});

test("parent links resolve through non-message entries and dangling ids chain linearly", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "first")),
      line({
        type: "file-history-snapshot",
        messageId: "u1",
        snapshot: {},
        uuid: "snap-1",
        parentUuid: "u1",
      }),
      line(userEntry("u2", "snap-1", "after snapshot")),
      line(userEntry("u3", "uuid-that-does-not-exist", "dangling")),
    ].join("\n"),
    { id: "s" },
  );
  // the snapshot emitted no node but sits in the uuid chain
  expect(session.nodes.map((n) => n.role)).toEqual(["user", "user", "user"]);
  expect(Option.getOrUndefined(session.nodes[1].parentNodeId)).toBe(0);
  expect(Option.getOrUndefined(session.nodes[2].parentNodeId)).toBe(1);
});

test("edit-family tool calls carry locations and revertable diffs", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "change things")),
      line(
        assistantEntry("u2", "u1", [
          {
            type: "tool_use",
            id: "toolu_e",
            name: "Edit",
            input: { file_path: "/work/proj/a.ts", old_string: "old", new_string: "new" },
          },
          {
            type: "tool_use",
            id: "toolu_m",
            name: "MultiEdit",
            input: {
              file_path: "/work/proj/b.ts",
              edits: [
                { old_string: "o1", new_string: "n1" },
                { old_string: "o2" },
                { not: "a-hunk" },
              ],
            },
          },
          {
            type: "tool_use",
            id: "toolu_w",
            name: "Write",
            input: { file_path: "/work/proj/c.ts", content: "whole file" },
          },
          {
            type: "tool_use",
            id: "toolu_r",
            name: "Read",
            input: { file_path: "/work/proj/a.ts" },
          },
          {
            type: "tool_use",
            id: "toolu_n",
            name: "NotebookEdit",
            input: { notebook_path: "/work/proj/nb.ipynb", new_source: "cell" },
          },
          { type: "tool_use", id: "toolu_b", name: "Bash", input: { command: "ls" } },
          { type: "tool_use", id: "toolu_g", name: "Glob", input: { path: "/work/proj" } },
          // degenerate inputs stay honest: nothing recorded is better than
          // guessing a diff.
          { type: "tool_use", id: "toolu_raw", name: "Edit", input: "not-an-object" },
          {
            type: "tool_use",
            id: "toolu_one",
            name: "Edit",
            input: { file_path: "/work/proj/d.ts", new_string: "only-new" },
          },
          {
            type: "tool_use",
            id: "toolu_bad_edits",
            name: "MultiEdit",
            input: { file_path: "/work/proj/e.ts", edits: "not-a-list" },
          },
          {
            type: "tool_use",
            id: "toolu_junk_edit",
            name: "MultiEdit",
            input: { file_path: "/work/proj/e.ts", edits: ["junk", { new_string: "x" }] },
          },
          {
            type: "tool_use",
            id: "toolu_no_content",
            name: "Write",
            input: { file_path: "/work/proj/f.ts" },
          },
          { type: "redacted_thinking" },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const [
    edit,
    multi,
    write,
    read,
    notebook,
    bash,
    glob,
    raw,
    oneSided,
    badEdits,
    junkEdits,
    noContent,
  ] = session.nodes[1].toolCalls;
  expect(edit.locations).toEqual([{ path: "/work/proj/a.ts" }]);
  expect(edit.diffs).toEqual([{ path: "/work/proj/a.ts", oldText: "old", newText: "new" }]);
  expect(multi.diffs).toEqual([
    { path: "/work/proj/b.ts", oldText: "o1", newText: "n1" },
    { path: "/work/proj/b.ts", oldText: "o2" },
  ]);
  expect(write.diffs).toEqual([{ path: "/work/proj/c.ts", newText: "whole file" }]);
  expect(read.locations).toEqual([{ path: "/work/proj/a.ts" }]);
  expect(read.diffs).toEqual([]);
  // cell-level new_source is not a file diff — location only
  expect(notebook.locations).toEqual([{ path: "/work/proj/nb.ipynb" }]);
  expect(notebook.diffs).toEqual([]);
  expect(bash.locations).toEqual([]);
  expect(bash.diffs).toEqual([]);
  expect(glob.locations).toEqual([{ path: "/work/proj" }]);
  expect(raw.locations).toEqual([]);
  expect(raw.diffs).toEqual([]);
  expect(oneSided.diffs).toEqual([{ path: "/work/proj/d.ts", newText: "only-new" }]);
  expect(badEdits.diffs).toEqual([]);
  expect(junkEdits.diffs).toEqual([{ path: "/work/proj/e.ts", newText: "x" }]);
  expect(noContent.diffs).toEqual([]);
  expect(noContent.locations).toEqual([{ path: "/work/proj/f.ts" }]);
});

test("file-history-snapshot entries become checkpoints with a path→backup map", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "go")),
      line({
        type: "file-history-snapshot",
        messageId: "msg-1",
        uuid: "snap-1",
        parentUuid: "u1",
        snapshot: {
          messageId: "msg-1",
          timestamp: "2026-01-01T00:00:00.500Z",
          trackedFileBackups: {
            "/work/proj/a.ts": {
              backupFileName: "hash1@v1",
              version: 1,
              backupTime: "2026-01-01T00:00:00.000Z",
            },
            "/work/proj/deleted.ts": { backupFileName: null, version: 2 },
            "/work/proj/junk.ts": "not-an-object",
          },
        },
      }),
      // an isSnapshotUpdate entry tops up the same ref — files merge
      line({
        type: "file-history-snapshot",
        messageId: "msg-1",
        isSnapshotUpdate: true,
        uuid: "snap-2",
        parentUuid: "snap-1",
        snapshot: {
          messageId: "msg-1",
          trackedFileBackups: {
            "/work/proj/b.ts": { backupFileName: "hash2@v1", version: 1 },
          },
        },
      }),
      line(userEntry("u2", "snap-2", "next")),
    ].join("\n"),
    { id: "s" },
  );
  expect(session.checkpoints).toEqual([
    { ref: "msg-1", createdAt: 1767225600000, kind: "file-history-snapshot" },
  ]);
  const history = (session.metadata as any).fileHistory;
  expect(history.sessionId).toBe("sess-1");
  expect(history.snapshots["msg-1"].files).toEqual({
    "/work/proj/a.ts": { backup: "hash1@v1", version: 1 },
    "/work/proj/deleted.ts": { backup: null, version: 2 },
    "/work/proj/b.ts": { backup: "hash2@v1", version: 1 },
  });
});

test("file-history falls back to the file id and tolerates missing pieces", () => {
  const session = ClaudeCode.fromJsonl(
    [
      // no sessionId/timestamp anywhere — uuid is the ref, epoch the time
      line({
        type: "file-history-snapshot",
        uuid: "snap-x",
        snapshot: {
          trackedFileBackups: {
            "/w/a.ts": { backupFileName: "h@v1" },
          },
        },
      }),
      // no ids at all — nothing to key the snapshot under
      line({ type: "file-history-snapshot", parentUuid: "snap-x" }),
      // ref inside the snapshot object; a non-map trackedFileBackups is empty
      line({
        type: "file-history-snapshot",
        uuid: "snap-y",
        snapshot: { messageId: "inner-y", trackedFileBackups: "junk" },
      }),
    ].join("\n"),
    { id: "sid-fallback" },
  );
  expect(session.checkpoints).toEqual([
    { ref: "snap-x", createdAt: 0, kind: "file-history-snapshot" },
    { ref: "inner-y", createdAt: 0, kind: "file-history-snapshot" },
  ]);
  const history = (session.metadata as any).fileHistory;
  expect(history.sessionId).toBe("sid-fallback");
  expect(history.snapshots["snap-x"].files).toEqual({
    "/w/a.ts": { backup: "h@v1" },
  });
  expect(history.snapshots["inner-y"].files).toEqual({});
});

test("sessions without file history expose no checkpoint metadata", () => {
  const session = ClaudeCode.fromJsonl(line(userEntry("u1", null, "hi")), { id: "s" });
  expect(session.checkpoints).toEqual([]);
  expect((session.metadata as any).fileHistory).toBeUndefined();
});

test("entries without parentUuid chain linearly; self-parent loops are safe", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({ type: "user", uuid: "u1", message: { content: "one" } }),
      line({ type: "user", uuid: "u2", parentUuid: "u2", message: { content: "two" } }),
    ].join("\n"),
    { id: "s" },
  );
  expect(Option.isNone(session.nodes[0].parentNodeId)).toBe(true);
  expect(Option.getOrUndefined(session.nodes[1].parentNodeId)).toBe(0);
});

test("malformed and non-object lines are skipped", () => {
  const session = ClaudeCode.fromJsonl(
    ['{"type":"user"', '"just a string"', "", "   ", line(userEntry("u1", null, "hi"))].join("\n"),
    { id: "s" },
  );
  expect(session.nodes).toHaveLength(1);
  expect(session.nodes[0].content).toBe("hi");
});

test("isMeta user entries stay out of promptHistory but remain nodes", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(
        userEntry("u1", null, "<local-command-caveat>ran /clear</local-command-caveat>", {
          isMeta: true,
        }),
      ),
      line(userEntry("u2", "u1", "real prompt")),
    ].join("\n"),
    { id: "s" },
  );
  expect(session.nodes).toHaveLength(2);
  expect((session.nodes[0].metadata as any).isMeta).toBe(true);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["real prompt"]);
});

test("user entries without content emit no node; assistant text/thinking/redacted blocks fold", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({ type: "user", uuid: "u0", parentUuid: null, message: { role: "user" } }),
      line(userEntry("u1", "u0", "go")),
      line(
        assistantEntry("u2", "u1", [
          { type: "redacted_thinking", data: "opaque" },
          { type: "thinking", thinking: "plan" },
          { type: "text", text: "answer" },
          42,
          { type: "tool_use", name: "NoId" },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  // the content-less user entry emitted nothing but stays in the uuid chain
  expect(session.nodes).toHaveLength(2);
  const assistant = session.nodes[1];
  expect(assistant.content).toBe("answer");
  // the redacted block folds to the marker and its opaque blob rides as the
  // thinking signature — the unsealed `thinking` block adds only its text.
  expect(Option.getOrUndefined(assistant.thinking)).toBe("[redacted]\nplan");
  expect(Option.getOrUndefined(assistant.thinkingSignature)).toBe("opaque");
  expect(assistant.toolCalls[0].id).toMatch(/^claude-tool-/);
  expect(assistant.toolCalls[0].name).toBe("NoId");
  expect(assistant.toolCalls[0].arguments).toEqual({});
});

test("assistant without usage or model leaves the options empty", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line({
        type: "assistant",
        uuid: "u2",
        parentUuid: "u1",
        timestamp: "not-a-date",
        message: { role: "assistant", content: [{ type: "text", text: "ok" }] },
      }),
    ].join("\n"),
    { id: "s" },
  );
  const assistant = session.nodes[1];
  expect(Option.isNone(assistant.usage)).toBe(true);
  expect(Option.isNone(assistant.model)).toBe(true);
  expect(Option.isNone(assistant.finishReason)).toBe(true);
  expect(Option.isNone(assistant.requestId)).toBe(true);
  // bad timestamp falls back to the session's first timestamp
  expect(assistant.createdAt).toBe(session.createdAt);
});

test("summarizeJsonl gives session meta without nodes", () => {
  const summary = ClaudeCode.summarizeJsonl(
    [
      line({ type: "summary", summary: "Listed", leafUuid: "u1" }),
      line(userEntry("u1", null, "do things")),
      line(assistantEntry("u2", "u1", [{ type: "text", text: "done" }])),
    ].join("\n"),
    { id: "sess-1" },
  );
  expect(summary.id).toBe("sess-1");
  expect(summary.title).toBe("Listed");
  expect(summary.workingDirectory).toBe("/work/proj");
  expect(summary.model).toBe("claude-opus-4-5");
  expect(summary.nodes).toEqual([]);
  expect(summary.mainChainId).toBe(0);
});

test("decodeProjectDir maps slugs back to paths", () => {
  expect(ClaudeCode.decodeProjectDir("-home-luis-proj")).toBe("/home/luis/proj");
  expect(ClaudeCode.decodeProjectDir("bare")).toBe("/bare");
});

test("user entries with no usable text emit no node and no history entry", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line({ type: "user", uuid: "u0", parentUuid: null, message: { content: "" } }),
      line({ type: "user", uuid: "u1", parentUuid: "u0", message: { content: [] } }),
      line({ type: "user", uuid: "u2", parentUuid: "u1", message: { content: 7 } }),
      line({ type: "user", uuid: "u3", parentUuid: "u2" }),
      line(userEntry("u4", "u3", "real")),
      line({ type: "user", parentUuid: "u4", message: { content: "uuid-less" } }),
    ].join("\n"),
    { id: "s" },
  );
  expect(session.nodes.map((n) => n.content)).toEqual(["real", "uuid-less"]);
  expect(session.promptHistory.map((p) => p.content)).toEqual(["real", "uuid-less"]);
  // the uuid-less node chains to the last emitted node
  expect(Option.getOrUndefined(session.nodes[1].parentNodeId)).toBe(0);
});

test("assistant entries without a message object emit an empty node", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line({ type: "assistant", uuid: "u2", parentUuid: "u1" }),
    ].join("\n"),
    { id: "s" },
  );
  const assistant = session.nodes[1];
  expect(assistant.role).toBe("assistant");
  expect(assistant.content).toBe("");
  expect(Option.isNone(assistant.usage)).toBe(true);
});

test("assistant usage object without token counts yields none", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "hi")),
      line(
        assistantEntry("u2", "u1", [{ type: "text", text: "ok" }], {
          usage: { service_tier: "standard" },
        }),
      ),
    ].join("\n"),
    { id: "s" },
  );
  expect(Option.isNone(session.nodes[1].usage)).toBe(true);
});

test("sanitization strips control characters from content", () => {
  const session = ClaudeCode.fromJsonl(line(userEntry("u1", null, "clean\x01me")), { id: "s" });
  expect(session.nodes[0].content).toBe("cleanme");
});

test("numeric parentUuid and non-string fields degrade to linear chaining", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "one")),
      line({ type: "user", uuid: "u2", parentUuid: 3, message: { content: "two" } }),
    ].join("\n"),
    { id: "s" },
  );
  expect(Option.getOrUndefined(session.nodes[1].parentNodeId)).toBe(0);
});

test("system entries without content or subtype get a generic label", () => {
  const session = ClaudeCode.fromJsonl(line({ type: "system", uuid: "s0", parentUuid: null }), {
    id: "s",
  });
  expect(session.nodes[0].role).toBe("system");
  expect(session.nodes[0].content).toBe("[claude system]");
  expect((session.nodes[0].metadata as any).subtype).toBeNull();
});

test("tool_use blocks without name or id still produce a call", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "go")),
      line(assistantEntry("u2", "u1", [{ type: "tool_use", input: { x: 1 } }])),
    ].join("\n"),
    { id: "s" },
  );
  const call = session.nodes[1].toolCalls[0];
  expect(call.name).toBe("unknown");
  expect(call.id).toMatch(/^claude-tool-/);
});

test("permissionMode on entries becomes agentMode; isCompactSummary is kept", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(
        userEntry("u1", null, "hi", {
          permissionMode: "plan",
          isCompactSummary: true,
          isVisibleInTranscriptOnly: true,
        }),
      ),
    ].join("\n"),
    { id: "s" },
  );
  expect(session.agentMode).toBe("plan");
  const meta = session.nodes[0].metadata as any;
  expect(meta.isCompactSummary).toBe(true);
  expect(meta.isVisibleInTranscriptOnly).toBe(true);
});

test("toolUseResult sidecar rides on the tool node metadata", () => {
  const session = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "go")),
      line(
        assistantEntry("u2", "u1", [
          { type: "tool_use", id: "toolu_t", name: "Task", input: { prompt: "x" } },
        ]),
      ),
      line(
        userEntry(
          "u3",
          "u2",
          [{ type: "tool_result", tool_use_id: "toolu_t", content: "spawned" }],
          { toolUseResult: { status: "completed", agentId: "agent-1" } },
        ),
      ),
    ].join("\n"),
    { id: "s" },
  );
  expect((session.nodes[2].metadata as any).toolUseResult).toEqual({
    status: "completed",
    agentId: "agent-1",
  });
});

// --- repository ---

// The repository builds its own BunFileSystem/Path layer (same convention as
// ClineRepository), so tests exercise it against a real temp dir.
const makeStore = (files: Record<string, string>): string => {
  const root = mkdtempSync(join(tmpdir(), "sepia-claude-"));
  for (const [rel, content] of Object.entries(files)) {
    const filePath = join(root, rel);
    mkdirSync(dirname(filePath), { recursive: true });
    writeFileSync(filePath, content);
  }
  return root;
};

const mainTranscript = [
  line({ type: "summary", summary: "Main session", leafUuid: "u1" }),
  line(userEntry("u1", null, "main prompt")),
].join("\n");

/** `session` re-keyed — make() needs plain props, not a class spread. */
const rekey = (
  session: Session,
  over: {
    readonly id?: string;
    readonly parentSessionId?: Option.Option<string>;
    readonly agentId?: Option.Option<string>;
  },
): Session =>
  Session.make({
    id: over.id ?? session.id,
    title: session.title,
    workingDirectory: session.workingDirectory,
    backendType: session.backendType,
    agentMode: session.agentMode,
    model: session.model,
    createdAt: session.createdAt,
    lastActivityAt: session.lastActivityAt,
    mainChainId: session.mainChainId,
    shellLastSeenIndex: session.shellLastSeenIndex,
    cogsJson: session.cogsJson,
    workspaceDirs: session.workspaceDirs,
    hidden: session.hidden,
    parentSessionId: over.parentSessionId ?? session.parentSessionId,
    agentId: over.agentId ?? session.agentId,
    checkpoints: session.checkpoints,
    metadata: session.metadata,
    nodes: session.nodes,
    promptHistory: session.promptHistory,
  });

const subagentTranscript = [
  line({
    type: "user",
    uuid: "a1",
    parentUuid: null,
    sessionId: "sess-1",
    agentId: "agent-9",
    isSidechain: true,
    cwd: "/work/proj",
    timestamp: "2026-01-01T00:00:05.000Z",
    message: { role: "user", content: "sub task" },
  }),
].join("\n");

test("repository lists main, legacy and subagents/ transcripts", async () => {
  const projectsDir = makeStore({
    "-work-proj/sess-1.jsonl": mainTranscript,
    // legacy layout: agent-*.jsonl sits beside the parent's file
    "-work-proj/agent-9.jsonl": subagentTranscript,
    // current layout: <parent-uuid>/subagents/agent-*.jsonl
    "-work-proj/sess-2/subagents/agent-7.jsonl": subagentTranscript,
    "-work-proj/sess-2/subagents/notes.txt": "not a transcript",
    "-work-proj/empty.jsonl": "",
    "-work-proj/notes.txt": "ignored",
    "-work-proj/attic/readme.md": "not a transcript dir",
  });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  const sessions = await Effect.runPromise(repo.list());

  const byId = new Map(sessions.map((s) => [s.id, s]));
  expect(byId.get("sess-1")?.title).toBe("Main session");
  expect(byId.get("sess-1")?.workingDirectory).toBe("/work/proj");
  // legacy subagent links to its parent through the entries' sessionId
  expect(Option.getOrUndefined(byId.get("agent-9")?.parentSessionId ?? Option.none())).toBe(
    "sess-1",
  );
  // new-layout subagent also carries the parent uuid from its directory name
  expect(Option.getOrUndefined(byId.get("agent-7")?.parentSessionId ?? Option.none())).toBe(
    "sess-1",
  );
  expect(Option.getOrUndefined(byId.get("agent-7")?.agentId ?? Option.none())).toBe("agent-9");
  expect(byId.has("empty")).toBe(false);
  // summaries carry no nodes
  expect(byId.get("sess-1")?.nodes).toEqual([]);
});

test("repository treats a missing projects dir as empty", async () => {
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({
    projectsDir: join(tmpdir(), "sepia-claude-does-not-exist"),
  });
  expect(await Effect.runPromise(repo.list())).toEqual([]);
});

test("repository getById parses the full transcript; unknown ids are none", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": mainTranscript });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });

  const found = await Effect.runPromise(repo.getById("sess-1"));
  expect(Option.isSome(found)).toBe(true);
  const session = Option.getOrThrow(found) as Session;
  expect(session.nodes.length).toBeGreaterThan(0);
  expect(session.nodes[0].content).toBe("main prompt");

  expect(Option.isNone(await Effect.runPromise(repo.getById("ghost")))).toBe(true);
});

test("repository hasSession reflects the files on disk", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": mainTranscript });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  await expect(Effect.runPromise(repo.hasSession("sess-1"))).resolves.toBe(true);
  await expect(Effect.runPromise(repo.hasSession("nope"))).resolves.toBe(false);
});

test("repository skips unstatable, unreadable and non-dir entries", async () => {
  const projectsDir = makeStore({
    "-work-proj/sess-1.jsonl": mainTranscript,
    // a `subagents` path that is a file, not a directory
    "-work-proj/sess-3/subagents": "not a dir",
    // a `.jsonl` that is actually a directory — unreadable as a file
    "-work-proj/dir.jsonl/x": "x",
    // a stray non-dir entry at the projects root
    "stray.txt": "loose file",
  });
  // a project-dir entry whose stat fails (dangling symlink)
  symlinkSync(join(projectsDir, "ghost-target"), join(projectsDir, "-broken"));

  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  const sessions = await Effect.runPromise(repo.list());
  expect(sessions.map((s) => s.id)).toEqual(["sess-1"]);

  // the .jsonl "file" that is a directory is skipped on list and
  // StorageError on getById (EISDIR is not a not-found)
  await expect(Effect.runPromise(repo.getById("dir"))).rejects.toThrow(
    "Failed to read claude session",
  );
});

test("repository tolerates an unreadable project dir and a vanished transcript", async () => {
  const projectsDir = makeStore({ "-locked/sess-1.jsonl": mainTranscript });
  chmodSync(join(projectsDir, "-locked"), 0o000);
  try {
    const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
    // inner readDirectory failure degrades that project to empty
    expect(await Effect.runPromise(repo.list())).toEqual([]);

    // the file shows up in the scan but is gone/unreadable at read time:
    // not-found errors map to none, other failures surface as StorageError
    chmodSync(join(projectsDir, "-locked"), 0o755);
    rmSync(join(projectsDir, "-locked/sess-1.jsonl"));
    symlinkSync(join(projectsDir, "-locked/ghost"), join(projectsDir, "-locked/sess-1.jsonl"));
    expect(Option.isNone(await Effect.runPromise(repo.getById("sess-1")))).toBe(true);
  } finally {
    chmodSync(join(projectsDir, "-locked"), 0o755);
  }
});

test("repository list and hasSession fail when the root cannot be listed", async () => {
  // a projectsDir that is a file: exists() is true, readDirectory throws
  const root = mkdtempSync(join(tmpdir(), "sepia-claude-root-"));
  const notADir = join(root, "projects");
  writeFileSync(notADir, "nope");
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir: notADir });
  await expect(Effect.runPromise(repo.list())).rejects.toThrow("Failed to list claude sessions");
  await expect(Effect.runPromise(repo.hasSession("x"))).rejects.toThrow(
    "Failed to check claude session",
  );
});

test("toJsonl writes a transcript fromJsonl reads back", () => {
  const original = ClaudeCode.fromJsonl(
    [
      line({ type: "summary", summary: "Fix the login bug", leafUuid: "u4" }),
      line(userEntry("u1", null, "fix the login bug please")),
      line(
        assistantEntry("u2", "u1", [
          { type: "thinking", thinking: "look at auth", signature: "sig-1" },
          { type: "text", text: "I'll check the auth module" },
          { type: "tool_use", id: "toolu_1", name: "Read", input: { file_path: "/src/a.ts" } },
        ]),
      ),
      line(
        userEntry("u3", "u2", [
          { type: "tool_result", tool_use_id: "toolu_1", content: "file contents" },
        ]),
      ),
      line(
        assistantEntry("u4", "u3", [{ type: "text", text: "done" }], {
          usage: { input_tokens: 5, output_tokens: 2 },
        }),
      ),
    ].join("\n"),
    { id: "sess-1" },
  );

  const written = ClaudeCode.toJsonl(original);
  // every line is a parseable entry with a uuid chain
  const entries = written
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as Record<string, unknown>);
  expect(entries[0].type).toBe("summary");
  expect(entries[0].summary).toBe("Fix the login bug");
  const uuids = entries.filter((e) => e.type !== "summary").map((e) => e.uuid);
  expect(new Set(uuids).size).toBe(uuids.length);

  const reread = ClaudeCode.fromJsonl(written, { id: "sess-1" });
  expect(reread.title).toBe("Fix the login bug");
  expect(reread.workingDirectory).toBe("/work/proj");
  expect(reread.model).toBe("claude-opus-4-5");
  expect(reread.nodes.map((n) => n.role)).toEqual(["user", "assistant", "tool", "assistant"]);

  const [user, assistant, tool, last] = reread.nodes;
  expect(user.content).toBe("fix the login bug please");
  expect(Option.isNone(user.parentNodeId)).toBe(true);
  // recorded uuids survive, so the parent links are identical
  expect((user.metadata as any).uuid).toBe("u1");
  expect((assistant.metadata as any).uuid).toBe("u2");
  expect(Option.getOrUndefined(assistant.parentNodeId)).toBe(user.nodeId);
  expect(Option.getOrUndefined(assistant.thinking)).toBe("look at auth");
  expect(Option.getOrUndefined(assistant.thinkingSignature)).toBe("sig-1");
  expect(assistant.toolCalls[0]).toMatchObject({
    id: "toolu_1",
    name: "Read",
    arguments: { file_path: "/src/a.ts" },
  });
  expect(Option.getOrUndefined(assistant.usage)).toEqual({
    input: 100,
    output: 40,
    cacheRead: 12,
    cacheWrite: 8,
  });
  expect(tool.role).toBe("tool");
  expect(Option.getOrUndefined(tool.toolCallId)).toBe("toolu_1");
  expect(tool.content).toBe("file contents");
  expect(Option.getOrUndefined(last.usage)).toEqual({ input: 5, output: 2 });
  expect(reread.promptHistory.map((p) => p.content)).toEqual(["fix the login bug please"]);
});

test("toJsonl marks subagent sessions isSidechain with the parent sessionId", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": mainTranscript });
  const sub = await Effect.runPromise(
    ClaudeCode.fromFile(join(projectsDir, "-work-proj/sess-1.jsonl")).pipe(
      Effect.provide(Layer.merge(BunFileSystem.layer, Path.layer)),
    ),
  );
  const subagent = rekey(sub, {
    id: "agent-9",
    parentSessionId: Option.some("sess-1"),
    agentId: Option.some("agent-9"),
  });
  const entries = ClaudeCode.toJsonl(subagent)
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as Record<string, unknown>);
  for (const entry of entries) {
    if (entry.type === "summary") continue;
    expect(entry.isSidechain).toBe(true);
    expect(entry.sessionId).toBe("sess-1");
    expect(entry.agentId).toBe("agent-9");
  }
});

test("toJsonl drops unsigned thinking; a redacted marker echoes its blob", () => {
  const unsigned = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "go")),
      line(
        assistantEntry("u2", "u1", [
          { type: "thinking", thinking: "unsigned plan" },
          { type: "text", text: "answer" },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const assistant = ClaudeCode.toJsonl(unsigned)
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as Record<string, unknown>)
    .find((e) => e.type === "assistant");
  // no signature on the node — the thinking block can't replay, so it drops
  expect(assistant).toBeDefined();
  expect((assistant as { message: { content: unknown } }).message.content).toEqual([
    { type: "text", text: "answer" },
  ]);

  const redacted = ClaudeCode.fromJsonl(
    [
      line(userEntry("u1", null, "go")),
      line(
        assistantEntry("u2", "u1", [
          { type: "redacted_thinking", data: "opaque-blob" },
          { type: "text", text: "answer" },
        ]),
      ),
    ].join("\n"),
    { id: "s" },
  );
  const sealed = ClaudeCode.toJsonl(redacted)
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l) as Record<string, unknown>)
    .find((e) => e.type === "assistant");
  expect(sealed).toBeDefined();
  expect((sealed as { message: { content: unknown } }).message.content).toEqual([
    { type: "redacted_thinking", data: "opaque-blob" },
    { type: "text", text: "answer" },
  ]);
});

test("repository save writes the canonical layout and delete removes it", async () => {
  const projectsDir = makeStore({ "-work-proj/sess-1.jsonl": mainTranscript });
  const repo = ClaudeCodeRepository.makeClaudeCodeSessionRepository({ projectsDir });
  const layer = Layer.merge(BunFileSystem.layer, Path.layer);
  const session = await Effect.runPromise(
    ClaudeCode.fromFile(join(projectsDir, "-work-proj/sess-1.jsonl")).pipe(Effect.provide(layer)),
  );

  const copy = rekey(session, { id: "sess-copy" });
  await Effect.runPromise(repo.save(copy));
  const written = join(projectsDir, "-work-proj/sess-copy.jsonl");
  expect(existsSync(written)).toBe(true);
  await expect(Effect.runPromise(repo.hasSession("sess-copy"))).resolves.toBe(true);

  // a subagent lands under its parent's dir
  const subagent = rekey(session, {
    id: "agent-99",
    parentSessionId: Option.some("sess-1"),
  });
  await Effect.runPromise(repo.save(subagent));
  expect(existsSync(join(projectsDir, "-work-proj/sess-1/subagents/agent-99.jsonl"))).toBe(true);

  // delete removes the transcript and its subagents dir
  await Effect.runPromise(repo.delete("sess-1"));
  expect(existsSync(join(projectsDir, "-work-proj/sess-1.jsonl"))).toBe(false);
  expect(existsSync(join(projectsDir, "-work-proj/sess-1"))).toBe(false);
  await expect(Effect.runPromise(repo.hasSession("agent-99"))).resolves.toBe(false);

  await expect(Effect.runPromise(repo.save(rekey(session, { id: "a/b" })))).rejects.toThrow(
    "not a safe file name",
  );
});

test("fromFile resolves id, cwd and subagent parentage from the path", async () => {
  const projectsDir = makeStore({
    "-work-proj/sess-1.jsonl": mainTranscript,
    "-work-proj/sess-1/subagents/agent-9.jsonl": subagentTranscript,
  });
  const layer = Layer.merge(BunFileSystem.layer, Path.layer);

  const main = await Effect.runPromise(
    ClaudeCode.fromFile(join(projectsDir, "-work-proj/sess-1.jsonl")).pipe(Effect.provide(layer)),
  );
  expect(main.id).toBe("sess-1");
  expect(main.workingDirectory).toBe("/work/proj");

  const sub = await Effect.runPromise(
    ClaudeCode.fromFile(join(projectsDir, "-work-proj/sess-1/subagents/agent-9.jsonl")).pipe(
      Effect.provide(layer),
    ),
  );
  expect(sub.id).toBe("agent-9");
  expect(Option.getOrUndefined(sub.parentSessionId)).toBe("sess-1");

  await expect(
    Effect.runPromise(
      ClaudeCode.fromFile(join(projectsDir, "-work-proj/missing.jsonl")).pipe(
        Effect.provide(layer),
      ),
    ),
  ).rejects.toThrow("Claude Code transcript not found");
});
