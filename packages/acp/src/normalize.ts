import type {
  AcpSessionUpdate,
  PermissionOption,
  PermissionRequest,
  ToolCallDiff,
  ToolCallLocation,
} from "./types.js";

export const asRecord = (value: unknown): Record<string, unknown> =>
  typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};

export const asArray = (value: unknown): ReadonlyArray<unknown> =>
  Array.isArray(value) ? value : [];

export const asString = (value: unknown): string | undefined =>
  typeof value === "string" ? value : undefined;

export const asNumberOrNull = (value: unknown): number | null => {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "" && !Number.isNaN(Number(value))) {
    return Number(value);
  }
  return null;
};

const textOf = (content: unknown): string => asString(asRecord(content).text) ?? "";

const locationOf = (value: unknown): ToolCallLocation => {
  const loc = asRecord(value);
  const line = asNumberOrNull(loc.line);
  return { path: asString(loc.path) ?? "", ...(line === null ? {} : { line }) };
};

/**
 * `diff` entries of a tool call's `content` — `{type:"diff", path, oldText?,
 * newText?}`. Other content kinds (terminal output, wrapped content blocks)
 * are not file changes and stay out.
 */
const diffOf = (value: unknown): ToolCallDiff | null => {
  const c = asRecord(value);
  if (asString(c.type) !== "diff") return null;
  const path = asString(c.path);
  if (path === undefined) return null;
  const oldText = asString(c.oldText);
  const newText = asString(c.newText);
  return {
    path,
    ...(oldText === undefined ? {} : { oldText }),
    ...(newText === undefined ? {} : { newText }),
  };
};

const diffsOf = (content: unknown): ReadonlyArray<ToolCallDiff> =>
  asArray(content).flatMap((item) => {
    const diff = diffOf(item);
    return diff === null ? [] : [diff];
  });

const optionOf = (value: unknown): PermissionOption => {
  const option = asRecord(value);
  return {
    optionId: asString(option.optionId) ?? "",
    name: asString(option.name) ?? "",
    kind: asString(option.kind) ?? "",
  };
};

let permissionCounter = 0;

export const normalizeUpdate = (update: unknown): AcpSessionUpdate => {
  const u = asRecord(update);
  switch (asString(u.sessionUpdate)) {
    case "user_message_chunk":
      return { kind: "user_message_chunk", text: textOf(u.content) };
    case "agent_message_chunk":
      return { kind: "agent_message_chunk", text: textOf(u.content) };
    case "agent_thought_chunk":
      return { kind: "agent_thought_chunk", text: textOf(u.content) };
    case "tool_call":
      return {
        kind: "tool_call",
        toolCallId: asString(u.toolCallId) ?? "",
        title: asString(u.title) ?? "",
        status: asString(u.status) ?? "",
        toolKind: asString(u.kind) ?? "",
        rawInput: u.rawInput,
        locations: asArray(u.locations).map(locationOf),
        diffs: diffsOf(u.content),
      };
    case "tool_call_update": {
      const title = asString(u.title);
      const locations = asArray(u.locations).map(locationOf);
      const diffs = diffsOf(u.content);
      return {
        kind: "tool_call_update",
        toolCallId: asString(u.toolCallId) ?? "",
        status: asString(u.status) ?? "",
        ...(title === undefined ? {} : { title }),
        ...(u.rawInput === undefined ? {} : { rawInput: u.rawInput }),
        rawOutput: u.rawOutput,
        ...(locations.length === 0 ? {} : { locations }),
        ...(diffs.length === 0 ? {} : { diffs }),
      };
    }
    case "plan":
      return {
        kind: "plan",
        entries: asArray(u.entries).map((entry) => {
          const e = asRecord(entry);
          return { content: asString(e.content) ?? "", status: asString(e.status) ?? "" };
        }),
      };
    case "current_mode_update":
      return { kind: "current_mode_update", modeId: asString(u.currentModeId) ?? "" };
    case "available_commands_update":
      return {
        kind: "available_commands_update",
        commands: asArray(u.availableCommands).map((command) => {
          const c = asRecord(command);
          const description = asString(c.description);
          return description === undefined
            ? { name: asString(c.name) ?? "" }
            : { name: asString(c.name) ?? "", description };
        }),
      };
    default:
      return { kind: "other", sessionUpdate: asString(u.sessionUpdate) ?? "", raw: update };
  }
};

export const normalizePermission = (params: unknown): PermissionRequest => {
  const p = asRecord(params);
  const toolCall = asRecord(p.toolCall);
  const sessionId = asString(p.sessionId) ?? "";
  const toolCallId = asString(toolCall.toolCallId) ?? null;
  return {
    requestId: `${sessionId}:${toolCallId ?? "none"}:${permissionCounter++}`,
    sessionId,
    toolCallId,
    title: asString(toolCall.title) ?? "",
    options: asArray(p.options).map(optionOf),
  };
};
