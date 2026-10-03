import { expect, test } from "vite-plus/test";
import { EventType } from "@ag-ui/core";
import type { TextMessageContentEvent, ToolCallArgsEvent } from "@ag-ui/core";
import type { PermissionRequest } from "sepia-acp";
import { createTranslator } from "../src/Translator.js";

const types = (events: ReadonlyArray<{ type: EventType }>): EventType[] =>
  events.map((event) => event.type);

test("streams assistant text as start, content and end", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const events = [
    ...translator.startRun(),
    ...translator.translate({ kind: "agent_message_chunk", text: "Hello" }),
    ...translator.translate({ kind: "agent_message_chunk", text: ", world" }),
    ...translator.endTurn(),
  ];

  expect(types(events)).toEqual([
    EventType.RUN_STARTED,
    EventType.TEXT_MESSAGE_START,
    EventType.TEXT_MESSAGE_CONTENT,
    EventType.TEXT_MESSAGE_CONTENT,
    EventType.TEXT_MESSAGE_END,
    EventType.RUN_FINISHED,
  ]);

  const deltas = events
    .filter(
      (event): event is TextMessageContentEvent => event.type === EventType.TEXT_MESSAGE_CONTENT,
    )
    .map((event) => event.delta);
  expect(deltas).toEqual(["Hello", ", world"]);
});

test("closes text before a tool call and orders the tool events", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const events = [
    ...translator.startRun(),
    ...translator.translate({ kind: "agent_message_chunk", text: "working" }),
    ...translator.translate({
      kind: "tool_call",
      toolCallId: "call-1",
      title: "read_file",
      status: "pending",
      toolKind: "read",
      rawInput: { path: "/tmp/a" },
      locations: [],
    }),
    ...translator.translate({
      kind: "tool_call_update",
      toolCallId: "call-1",
      status: "completed",
      rawOutput: { ok: true },
    }),
  ];

  expect(types(events)).toEqual([
    EventType.RUN_STARTED,
    EventType.TEXT_MESSAGE_START,
    EventType.TEXT_MESSAGE_CONTENT,
    EventType.TEXT_MESSAGE_END,
    EventType.TOOL_CALL_START,
    EventType.TOOL_CALL_ARGS,
    EventType.TOOL_CALL_RESULT,
    EventType.TOOL_CALL_END,
  ]);

  const args = events.find(
    (event): event is ToolCallArgsEvent => event.type === EventType.TOOL_CALL_ARGS,
  );
  expect(args?.delta).toBe(JSON.stringify({ path: "/tmp/a" }));
});

test("maps a permission request to a single custom event", () => {
  const translator = createTranslator();
  const request: PermissionRequest = {
    requestId: "req-1",
    sessionId: "s-1",
    toolCallId: "call-1",
    title: "Allow?",
    options: [{ optionId: "yes", name: "Yes", kind: "allow_once" }],
  };

  const events = translator.permissionRequest(request);
  expect(events).toHaveLength(1);
  expect(events[0]).toMatchObject({
    type: EventType.CUSTOM,
    name: "acp:permission_request",
    value: request,
  });
});

test("maps plan, mode, commands and other updates to named custom events", () => {
  const translator = createTranslator();

  expect(
    translator.translate({ kind: "plan", entries: [{ content: "step", status: "pending" }] })[0],
  ).toMatchObject({ type: EventType.CUSTOM, name: "acp:plan" });
  expect(translator.translate({ kind: "current_mode_update", modeId: "code" })[0]).toMatchObject({
    type: EventType.CUSTOM,
    name: "acp:current_mode_update",
  });
  expect(
    translator.translate({ kind: "available_commands_update", commands: [{ name: "run" }] })[0],
  ).toMatchObject({ type: EventType.CUSTOM, name: "acp:available_commands_update" });
  expect(
    translator.translate({ kind: "other", sessionUpdate: "foo_bar", raw: { a: 1 } })[0],
  ).toMatchObject({ type: EventType.CUSTOM, name: "acp:foo_bar", value: { a: 1 } });
});

test("endTurn is idempotent about closing events", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });
  const first = [
    ...translator.startRun(),
    ...translator.translate({ kind: "agent_message_chunk", text: "hi" }),
    ...translator.translate({
      kind: "tool_call",
      toolCallId: "call-1",
      title: "t",
      status: "pending",
      toolKind: "other",
      rawInput: {},
      locations: [],
    }),
    ...translator.endTurn(),
  ];

  expect(first.filter((event) => event.type === EventType.TEXT_MESSAGE_END)).toHaveLength(1);
  expect(first.filter((event) => event.type === EventType.TOOL_CALL_END)).toHaveLength(1);
  expect(translator.endTurn()).toEqual([]);
});

test("ignores user chunks and streams reasoning", () => {
  const translator = createTranslator({ threadId: "t", runId: "r", messageId: "m" });

  expect(translator.translate({ kind: "user_message_chunk", text: "hi" })).toEqual([]);

  const events = [
    ...translator.translate({ kind: "agent_thought_chunk", text: "think" }),
    ...translator.translate({ kind: "agent_thought_chunk", text: "ing" }),
    ...translator.translate({ kind: "agent_message_chunk", text: "answer" }),
    ...translator.endTurn(),
  ];

  expect(types(events)).toEqual([
    EventType.REASONING_MESSAGE_START,
    EventType.REASONING_MESSAGE_CONTENT,
    EventType.REASONING_MESSAGE_CONTENT,
    EventType.REASONING_MESSAGE_END,
    EventType.TEXT_MESSAGE_START,
    EventType.TEXT_MESSAGE_CONTENT,
    EventType.TEXT_MESSAGE_END,
    EventType.RUN_FINISHED,
  ]);
});

test("never closes an unknown tool call", () => {
  const translator = createTranslator();
  expect(
    translator.translate({ kind: "tool_call_update", toolCallId: "ghost", status: "completed" }),
  ).toEqual([]);
});
