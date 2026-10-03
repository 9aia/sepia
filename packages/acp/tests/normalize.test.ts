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
    locations: [{ path: "/a" }],
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
