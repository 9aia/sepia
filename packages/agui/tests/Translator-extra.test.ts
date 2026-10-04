import { expect, test } from "vite-plus/test";
import { EventType } from "@ag-ui/core";
import type { ToolCallArgsEvent } from "@ag-ui/core";
import { createTranslator } from "../src/Translator.js";

const types = (events: ReadonlyArray<{ type: EventType }>): EventType[] =>
  events.map((event) => event.type);

test("a tool_call_update carrying rawInput emits another TOOL_CALL_ARGS", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const events = [
    ...translator.translate({
      kind: "tool_call",
      toolCallId: "call-1",
      title: "bash",
      status: "pending",
      toolKind: "execute",
      rawInput: { cmd: "ls" },
      locations: [],
      diffs: [],
    }),
    ...translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "in_progress",
      // `rawInput` rides along on real devin updates — a second ARGS frame.
      rawInput: { cmd: "ls -la" },
    }),
  ];
  const args = events.filter(
    (event): event is ToolCallArgsEvent => event.type === EventType.TOOL_CALL_ARGS,
  );
  expect(args).toHaveLength(2);
  expect(args[1]?.delta).toBe(JSON.stringify({ cmd: "ls -la" }));
});

test("a completed tool_call_update without rawOutput skips TOOL_CALL_RESULT", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const events = [
    ...translator.translate({
      kind: "tool_call",
      toolCallId: "call-1",
      title: "bash",
      status: "pending",
      toolKind: "execute",
      rawInput: {},
      locations: [],
      diffs: [],
    }),
    ...translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "completed",
    }),
  ];
  expect(types(events)).toEqual([
    EventType.TOOL_CALL_START,
    EventType.TOOL_CALL_ARGS,
    EventType.TOOL_CALL_END,
  ]);
});

test("a failed status also closes the tool call", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const events = [
    ...translator.translate({
      kind: "tool_call",
      toolCallId: "call-1",
      title: "bash",
      status: "pending",
      toolKind: "execute",
      rawInput: {},
      locations: [],
      diffs: [],
    }),
    ...translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "failed",
      rawOutput: { error: "denied" },
    }),
  ];
  expect(types(events)).toContain(EventType.TOOL_CALL_RESULT);
  expect(types(events)).toContain(EventType.TOOL_CALL_END);
});

test("an in-progress update on an open call emits no END", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  translator.translate({
    kind: "tool_call",
    toolCallId: "call-1",
    title: "bash",
    status: "pending",
    toolKind: "execute",
    rawInput: {},
    locations: [],
    diffs: [],
  });
  expect(
    translator.translate({ kind: "tool_call_update", toolCallId: "call-1", status: "running" }),
  ).toEqual([]);
});

test("forwards locations and diffs on the tool events", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const start = translator.translate({
    kind: "tool_call",
    toolCallId: "call-1",
    title: "edit_file",
    status: "pending",
    toolKind: "edit",
    rawInput: { path: "/a" },
    locations: [{ path: "/a", line: 2 }],
    diffs: [{ path: "/a", oldText: "x", newText: "y" }],
  });
  expect(start[0]).toMatchObject({
    type: EventType.TOOL_CALL_START,
    locations: [{ path: "/a", line: 2 }],
    diffs: [{ path: "/a", oldText: "x", newText: "y" }],
  });

  const end = translator.translate({
    kind: "tool_call_update",
    toolCallId: "call-1",
    status: "completed",
    diffs: [{ path: "/b", newText: "created" }],
  });
  expect(end.at(-1)).toMatchObject({
    type: EventType.TOOL_CALL_END,
    status: "completed",
    diffs: [{ path: "/b", newText: "created" }],
  });
});

test("a mid-call file update rides a custom event", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  translator.translate({
    kind: "tool_call",
    toolCallId: "call-1",
    title: "edit_file",
    status: "pending",
    toolKind: "edit",
    rawInput: {},
    locations: [],
    diffs: [],
  });
  expect(
    translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "in_progress",
      locations: [{ path: "/a" }],
    }),
  ).toEqual([
    {
      type: EventType.CUSTOM,
      name: "acp:tool_call_update",
      value: { toolCallId: "call-1", locations: [{ path: "/a" }] },
    },
  ]);
});

test("startRun resets state so a second run mints fresh ids", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  translator.translate({ kind: "agent_message_chunk", text: "one" });
  const second = translator.startRun();
  expect(second).toEqual([{ type: EventType.RUN_STARTED, threadId: "t", runId: "r" }]);
  const events = translator.translate({ kind: "agent_message_chunk", text: "two" });
  // The open message from run 1 is gone — a new START opens instead of CONTENT.
  expect(events[0]?.type).toBe(EventType.TEXT_MESSAGE_START);
});
