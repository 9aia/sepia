import type { AgUiEvent } from "./api";

/** A message assembled from live AG-UI stream events (or an optimistic echo). */
export interface LiveMessage {
  readonly id: string;
  readonly createdAt?: number;
  readonly role: "user" | "assistant" | "reasoning" | "tool" | "status";
  readonly content: string;
  readonly toolName?: string;
  readonly done: boolean;
}

const update = (
  messages: ReadonlyArray<LiveMessage>,
  id: string | undefined,
  fn: (message: LiveMessage) => LiveMessage,
): LiveMessage[] => {
  if (id === undefined) return messages.slice();
  const index = messages.findIndex((message) => message.id === id);
  if (index === -1) return messages.slice();
  return messages.map((message, i) => (i === index ? fn(message) : message));
};

const append = (messages: ReadonlyArray<LiveMessage>, message: LiveMessage): LiveMessage[] =>
  messages.some((existing) => existing.id === message.id)
    ? messages.slice()
    : [...messages, message];

/**
 * Folds one AG-UI event into the live message list. Returns the previous
 * array identity when the event does not change anything so React can
 * skip re-renders.
 */
export function applyAguiEvent(
  messages: ReadonlyArray<LiveMessage>,
  event: AgUiEvent,
): LiveMessage[] {
  const messageId = typeof event.messageId === "string" ? event.messageId : undefined;
  const toolCallId = typeof event.toolCallId === "string" ? event.toolCallId : undefined;
  const delta = typeof event.delta === "string" ? event.delta : "";

  switch (event.type) {
    case "RUN_FINISHED":
      return [
        ...messages,
        { id: `status-${Date.now()}`, role: "status", content: "Run finished", done: true },
      ];
    case "RUN_ERROR":
      return [
        ...messages,
        { id: `status-${Date.now()}`, role: "status", content: "Run failed", done: true },
      ];
    case "TEXT_MESSAGE_START":
      return append(messages, {
        id: messageId ?? `text-${messages.length}`,
        role: "assistant",
        content: "",
        done: false,
      });
    case "TEXT_MESSAGE_CONTENT":
      return update(messages, messageId, (m) => ({ ...m, content: m.content + delta }));
    case "TEXT_MESSAGE_END":
      return update(messages, messageId, (m) => ({ ...m, done: true }));
    case "REASONING_MESSAGE_START":
      return append(messages, {
        id: messageId ?? `reasoning-${messages.length}`,
        role: "reasoning",
        content: "",
        done: false,
      });
    case "REASONING_MESSAGE_CONTENT":
      return update(messages, messageId, (m) => ({ ...m, content: m.content + delta }));
    case "REASONING_MESSAGE_END":
      return update(messages, messageId, (m) => ({ ...m, done: true }));
    case "TOOL_CALL_START":
      return append(messages, {
        id: toolCallId ?? `tool-${messages.length}`,
        role: "tool",
        toolName: typeof event.toolCallName === "string" ? event.toolCallName : undefined,
        content: "",
        done: false,
      });
    case "TOOL_CALL_ARGS":
      return update(messages, toolCallId, (m) => ({ ...m, content: m.content + delta }));
    case "TOOL_CALL_RESULT": {
      const content =
        typeof event.content === "string" ? event.content : JSON.stringify(event.content ?? "");
      return update(messages, toolCallId, (m) => ({ ...m, content: m.content + content }));
    }
    case "TOOL_CALL_END":
      return update(messages, toolCallId, (m) => ({ ...m, done: true }));
    default:
      return messages.slice();
  }
}
