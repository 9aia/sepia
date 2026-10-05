import { expect, test } from "vite-plus/test";
import { normalizePermission, normalizeUpdate } from "../src/normalize.js";

test("normalizes the three message chunk variants", () => {
  const content = { type: "text", text: "hello" };
  expect(normalizeUpdate({ sessionUpdate: "user_message_chunk", content })).toEqual({
    kind: "user_message_chunk",
    text: "hello",
  });
  expect(normalizeUpdate({ sessionUpdate: "agent_message_chunk", content })).toEqual({
    kind: "agent_message_chunk",
    text: "hello",
  });
  expect(normalizeUpdate({ sessionUpdate: "agent_thought_chunk", content })).toEqual({
    kind: "agent_thought_chunk",
    text: "hello",
  });
});

test("normalizes tool_call", () => {
  expect(
    normalizeUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "Read file",
      status: "in_progress",
      kind: "read",
      rawInput: { path: "/a" },
      locations: [{ path: "/a", line: 3 }],
    }),
  ).toEqual({
    kind: "tool_call",
    toolCallId: "t1",
    title: "Read file",
    status: "in_progress",
    toolKind: "read",
    rawInput: { path: "/a" },
    locations: [{ path: "/a", line: 3 }],
    diffs: [],
  });
});

test("normalizes diff content on tool_call and tool_call_update", () => {
  const content = [
    { type: "diff", path: "/a", oldText: "x", newText: "y" },
    { type: "diff", path: "/b", newText: "new file" },
    { type: "content", content: { type: "text", text: "done" } },
    { type: "diff" },
  ];
  expect(
    normalizeUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "Edit file",
      status: "in_progress",
      kind: "edit",
      rawInput: {},
      content,
    }),
  ).toEqual({
    kind: "tool_call",
    toolCallId: "t1",
    title: "Edit file",
    status: "in_progress",
    toolKind: "edit",
    rawInput: {},
    locations: [],
    diffs: [
      { path: "/a", oldText: "x", newText: "y" },
      { path: "/b", newText: "new file" },
    ],
    contents: [{ type: "text", text: "done" }],
  });
  expect(
    normalizeUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "t1",
      status: "completed",
      content,
      locations: [{ path: "/a" }],
      rawInput: { path: "/a" },
    }),
  ).toEqual({
    kind: "tool_call_update",
    toolCallId: "t1",
    status: "completed",
    rawInput: { path: "/a" },
    rawOutput: undefined,
    locations: [{ path: "/a" }],
    diffs: [
      { path: "/a", oldText: "x", newText: "y" },
      { path: "/b", newText: "new file" },
    ],
    contents: [{ type: "text", text: "done" }],
  });
});

test("normalizes terminal and embedded content entries of a tool call", () => {
  const content = [
    { type: "terminal", terminalId: "term-1" },
    { type: "terminal", terminalId: "term-2", output: "build ok" },
    { type: "content", content: { type: "text", text: "result text" } },
    { type: "content", content: { type: "image", data: "aGk=", mimeType: "image/png" } },
    // Unsupported block kinds and malformed entries drop out.
    { type: "content", content: { type: "resource_link", uri: "file:///a" } },
    { type: "terminal" },
    { type: "diff", path: "/a", newText: "y" },
  ];
  expect(
    normalizeUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "exec",
      status: "in_progress",
      kind: "execute",
      rawInput: {},
      content,
    }),
  ).toEqual({
    kind: "tool_call",
    toolCallId: "t1",
    title: "exec",
    status: "in_progress",
    toolKind: "execute",
    rawInput: {},
    locations: [],
    diffs: [{ path: "/a", newText: "y" }],
    contents: [
      { type: "terminal", terminalId: "term-1" },
      { type: "terminal", terminalId: "term-2", output: "build ok" },
      { type: "text", text: "result text" },
      { type: "image", data: "aGk=", mimeType: "image/png" },
    ],
  });
});

test("normalizes tool_call_update with and without a title", () => {
  expect(
    normalizeUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "t1",
      status: "completed",
      title: "Done",
      rawOutput: { ok: true },
    }),
  ).toEqual({
    kind: "tool_call_update",
    toolCallId: "t1",
    status: "completed",
    title: "Done",
    rawOutput: { ok: true },
  });
  expect(normalizeUpdate({ sessionUpdate: "tool_call_update", toolCallId: "t1" })).toEqual({
    kind: "tool_call_update",
    toolCallId: "t1",
    status: "",
    rawOutput: undefined,
  });
});

test("normalizes plan", () => {
  expect(
    normalizeUpdate({
      sessionUpdate: "plan",
      entries: [
        { content: "step", status: "pending", priority: "high" },
        { content: "done", status: "completed", priority: "low" },
      ],
    }),
  ).toEqual({
    kind: "plan",
    entries: [
      { content: "step", status: "pending" },
      { content: "done", status: "completed" },
    ],
  });
});

test("normalizes current_mode_update", () => {
  expect(normalizeUpdate({ sessionUpdate: "current_mode_update", currentModeId: "plan" })).toEqual({
    kind: "current_mode_update",
    modeId: "plan",
  });
});

test("normalizes available_commands_update", () => {
  expect(
    normalizeUpdate({
      sessionUpdate: "available_commands_update",
      availableCommands: [{ name: "init", description: "Init the repo" }, { name: "help" }],
    }),
  ).toEqual({
    kind: "available_commands_update",
    commands: [{ name: "init", description: "Init the repo" }, { name: "help" }],
  });
});

test("falls back to other for unknown updates", () => {
  const update = { sessionUpdate: "notice", message: "hi" };
  expect(normalizeUpdate(update)).toEqual({ kind: "other", sessionUpdate: "notice", raw: update });
});

test("normalizes a permission request", () => {
  const request = normalizePermission({
    sessionId: "s1",
    toolCall: { toolCallId: "t1", title: "Run command" },
    options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
  });
  expect(request.sessionId).toBe("s1");
  expect(request.toolCallId).toBe("t1");
  expect(request.title).toBe("Run command");
  expect(request.options).toEqual([{ optionId: "allow", name: "Allow", kind: "allow_once" }]);
  expect(request.requestId).toMatch(/^s1:t1:\d+$/);
});

test("normalizes a permission request without a tool call", () => {
  const request = normalizePermission({ sessionId: "s2" });
  expect(request.toolCallId).toBeNull();
  expect(request.title).toBe("");
  expect(request.options).toEqual([]);
  expect(request.requestId).toMatch(/^s2:none:\d+$/);
});

test("normalization defaults missing fields to empty strings", () => {
  expect(normalizeUpdate({ sessionUpdate: "user_message_chunk", content: 5 })).toEqual({
    kind: "user_message_chunk",
    text: "",
  });
  expect(normalizeUpdate({ sessionUpdate: "tool_call" })).toMatchObject({
    kind: "tool_call",
    toolCallId: "",
    title: "",
    status: "",
    toolKind: "",
    locations: [],
    diffs: [],
  });
  expect(normalizeUpdate({ sessionUpdate: "tool_call_update" })).toMatchObject({
    kind: "tool_call_update",
    toolCallId: "",
    status: "",
  });
  expect(
    normalizeUpdate({ sessionUpdate: "plan", entries: [{ content: "x" }, { status: "done" }, {}] }),
  ).toEqual({
    kind: "plan",
    entries: [
      { content: "x", status: "" },
      { content: "", status: "done" },
      { content: "", status: "" },
    ],
  });
  expect(normalizeUpdate({ sessionUpdate: "current_mode_update" })).toEqual({
    kind: "current_mode_update",
    modeId: "",
  });
  expect(
    normalizeUpdate({
      sessionUpdate: "available_commands_update",
      availableCommands: [{ name: "a" }, { description: "b" }, {}],
    }),
  ).toEqual({
    kind: "available_commands_update",
    commands: [{ name: "a" }, { name: "", description: "b" }, { name: "" }],
  });
  expect(normalizeUpdate({})).toMatchObject({ kind: "other", sessionUpdate: "" });
});

test("content entries drop unusable shapes and keep partial ones", () => {
  const update = normalizeUpdate({
    sessionUpdate: "tool_call",
    toolCallId: "t",
    content: [
      // a content block that is a bare text
      { type: "content", content: { type: "text", text: "hi" } },
      // text block without text — dropped
      { type: "content", content: { type: "text" } },
      // image with no source — dropped
      { type: "content", content: { type: "image" } },
      // image with only a uri
      { type: "content", content: { type: "image", uri: "https://x.png" } },
      // image with data + mimeType
      { type: "content", content: { type: "image", data: "AAAA", mimeType: "image/png" } },
      // an unrecognized block — dropped
      { type: "content", content: { type: "audio", data: "x" } },
      // a bare diff entry with only a path
      { type: "diff", path: "/a.ts", newText: "n" },
      // a diff without a path — dropped
      { type: "diff", oldText: "o" },
      // a location with no line
      { type: "location" },
    ],
    locations: [{ path: "/b.ts" }],
  }) as { contents?: ReadonlyArray<unknown>; locations?: ReadonlyArray<unknown> };

  expect(update.contents).toEqual([
    { type: "text", text: "hi" },
    { type: "image", uri: "https://x.png" },
    { type: "image", data: "AAAA", mimeType: "image/png" },
  ]);
  expect(update.locations).toEqual([{ path: "/b.ts" }]);
});

test("normalizePermission defaults absent fields", () => {
  expect(normalizePermission({})).toMatchObject({
    sessionId: "",
    toolCallId: null,
    title: "",
    options: [],
  });
});
