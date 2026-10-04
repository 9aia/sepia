import type { AgUiEvent } from "./api";
import type {
  HistoryBlock,
  MessageUsage,
  ToolCallStatus,
  ToolFileDiff,
  ToolLocation,
} from "./types";

/** A message assembled from live AG-UI stream events (or an optimistic echo). */
export interface LiveMessage {
  readonly id: string;
  readonly createdAt?: number;
  readonly role: "user" | "assistant" | "reasoning" | "tool" | "status";
  readonly content: string;
  /** Attachment blocks on the optimistic user row (sent files render before history flushes). */
  readonly blocks?: ReadonlyArray<HistoryBlock>;
  /** Tool-call input JSON, kept apart from `content` (the result) so each can render on its own. */
  readonly args?: string;
  readonly toolName?: string;
  /** Files the call touched / changed — the ACP `locations`/`diffs` payload. */
  readonly locations?: ReadonlyArray<ToolLocation>;
  readonly diffs?: ReadonlyArray<ToolFileDiff>;
  /** Outcome of the tool call once the stream settles it (IR v2 fields ride along). */
  readonly toolStatus?: ToolCallStatus;
  readonly exitCode?: number;
  readonly durationMs?: number;
  /** Token metrics when the stream or history backfill carries them. */
  readonly usage?: MessageUsage;
  readonly model?: string;
  readonly finishReason?: string;
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

const num = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isFinite(v) ? v : undefined;

const nonEmpty = (v: unknown): string | undefined =>
  typeof v === "string" && v !== "" ? v : undefined;

const record = (v: unknown): Record<string, unknown> | undefined =>
  typeof v === "object" && v !== null ? (v as Record<string, unknown>) : undefined;

/** `{path, line?}` entries off a live event; undefined when nothing parses. */
const locationsOf = (v: unknown): ReadonlyArray<ToolLocation> | undefined => {
  if (!Array.isArray(v)) return undefined;
  const out: ToolLocation[] = [];
  for (const item of v) {
    const loc = record(item);
    const path = nonEmpty(loc?.["path"]);
    if (loc === undefined || path === undefined) continue;
    const line = num(loc["line"]);
    out.push(line === undefined ? { path } : { path, line });
  }
  return out.length === 0 ? undefined : out;
};

/** `{path, oldText?, newText?}` entries off a live event. */
const diffsOf = (v: unknown): ReadonlyArray<ToolFileDiff> | undefined => {
  if (!Array.isArray(v)) return undefined;
  const out: ToolFileDiff[] = [];
  for (const item of v) {
    const diff = record(item);
    const path = nonEmpty(diff?.["path"]);
    if (diff === undefined || path === undefined) continue;
    const oldText = typeof diff["oldText"] === "string" ? (diff["oldText"] as string) : undefined;
    const newText = typeof diff["newText"] === "string" ? (diff["newText"] as string) : undefined;
    out.push({
      path,
      ...(oldText === undefined ? {} : { oldText }),
      ...(newText === undefined ? {} : { newText }),
    });
  }
  return out.length === 0 ? undefined : out;
};

/**
 * File fields arriving on a tool event (or the `acp:tool_call_update`
 * custom event). A present snapshot replaces the row's — the agent sends
 * the call's current footprint, not a delta to append.
 */
const fileFields = (event: AgUiEvent | Record<string, unknown>) => {
  const locations = locationsOf(event["locations"]);
  const diffs = diffsOf(event["diffs"]);
  return {
    ...(locations === undefined ? {} : { locations }),
    ...(diffs === undefined ? {} : { diffs }),
  };
};

/** IR names win; ACP-style statuses map onto them. */
const toolStatusOf = (v: unknown): ToolCallStatus | undefined => {
  if (v === "success" || v === "completed") return "success";
  if (v === "error" || v === "failed") return "error";
  if (v === "pending" || v === "in_progress") return "pending";
  return undefined;
};

const usageOf = (v: unknown): MessageUsage | undefined => {
  if (typeof v !== "object" || v === null) return undefined;
  const record = v as Record<string, unknown>;
  const input = num(record["input"]);
  const output = num(record["output"]);
  if (input === undefined || output === undefined) return undefined;
  const usage: MessageUsage = { input, output };
  const cacheRead = num(record["cacheRead"]);
  const cacheWrite = num(record["cacheWrite"]);
  const thinking = num(record["thinking"]);
  const cost = num(record["cost"]);
  if (cacheRead !== undefined) usage.cacheRead = cacheRead;
  if (cacheWrite !== undefined) usage.cacheWrite = cacheWrite;
  if (thinking !== undefined) usage.thinking = thinking;
  if (cost !== undefined) usage.cost = cost;
  return usage;
};

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
    case "TEXT_MESSAGE_END": {
      const usage = usageOf(event.usage);
      const finishReason = nonEmpty(event.finishReason);
      const model = nonEmpty(event.model);
      return update(messages, messageId, (m) => ({
        ...m,
        done: true,
        ...(usage !== undefined ? { usage } : {}),
        ...(finishReason !== undefined ? { finishReason } : {}),
        ...(model !== undefined ? { model } : {}),
      }));
    }
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
        ...fileFields(event),
      });
    case "TOOL_CALL_ARGS":
      return update(messages, toolCallId, (m) => ({ ...m, args: (m.args ?? "") + delta }));
    case "TOOL_CALL_RESULT": {
      const content =
        typeof event.content === "string" ? event.content : JSON.stringify(event.content ?? "");
      return update(messages, toolCallId, (m) => ({ ...m, content: m.content + content }));
    }
    case "TOOL_CALL_END": {
      const toolStatus = toolStatusOf(event.toolStatus ?? event.status);
      const exitCode = num(event.exitCode);
      const durationMs = num(event.durationMs);
      return update(messages, toolCallId, (m) => ({
        ...m,
        done: true,
        ...(toolStatus !== undefined ? { toolStatus } : {}),
        ...(exitCode !== undefined ? { exitCode } : {}),
        ...(durationMs !== undefined ? { durationMs } : {}),
        ...fileFields(event),
      }));
    }
    case "CUSTOM": {
      // The Translator's mid-call file carrier — `{toolCallId, locations?,
      // diffs?}` — lands while the tool event stream is still open.
      if (event.name !== "acp:tool_call_update") return messages.slice();
      const value = record(event.value);
      const id = nonEmpty(value?.["toolCallId"]);
      if (value === undefined || id === undefined) return messages.slice();
      return update(messages, id, (m) => ({ ...m, ...fileFields(value) }));
    }
    default:
      return messages.slice();
  }
}
