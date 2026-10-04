import { expect, test } from "vite-plus/test";
import { EventType } from "@ag-ui/core";
import type { ToolCallArgsEvent } from "@ag-ui/core";
import type { AcpSessionUpdate } from "sepia-acp";
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
    }),
    ...translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "in_progress",
      // `rawInput` rides along on real devin updates although the frozen
      // AcpSessionUpdate type doesn't declare it — the translator reads it.
      rawInput: { cmd: "ls -la" },
    } as AcpSessionUpdate),
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
  });
  expect(
    translator.translate({ kind: "tool_call_update", toolCallId: "call-1", status: "running" }),
  ).toEqual([]);
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
