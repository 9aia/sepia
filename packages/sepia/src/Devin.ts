import { Option } from "effect";
import { randomUUID } from "node:crypto";
import { MessageNode, PromptHistoryEntry, Session, ToolCall } from "./Domain.js";

const toIso = (ts: number): string => new Date(ts * 1000).toISOString();

const toolCallDisplay = (tc: ToolCall) => {
  switch (tc.name) {
    case "read":
      return {
        title: "Read file",
        kind: "read",
        locations: [{ path: (tc.arguments as any)?.file_path ?? "" }],
      };
    case "exec":
      return { title: "Ran command", kind: "execute", locations: [] };
    case "grep":
      return { title: "Searched codebase", kind: "search", locations: [] };
    case "webfetch":
      return { title: "Fetched web content", kind: "fetch", locations: [] };
    case "edit":
      return {
        title: "Edited file",
        kind: "edit",
        locations: [{ path: (tc.arguments as any)?.file_path ?? "" }],
      };
    case "write":
      return {
        title: "Wrote file",
        kind: "edit",
        locations: [{ path: (tc.arguments as any)?.file_path ?? "" }],
      };
    default:
      return { title: tc.name, kind: "function", locations: [] };
  }
};

export const buildChatMessage = (node: MessageNode, generationModel: string): unknown => {
  const messageId = randomUUID();
  const base = { message_id: messageId, role: node.role, content: node.content };

  switch (node.role) {
    case "system":
      return base;
    case "user": {
      return {
        ...base,
        metadata: {
          num_tokens: null,
          is_user_input: true,
          request_id: null,
          metrics: null,
          finish_reason: null,
          extensions: {},
          created_at: toIso(node.createdAt),
          telemetry: { source: "user", operation: "input" },
        },
      };
    }
    case "assistant": {
      const nodeMeta = node.metadata as
        | { summarized_from?: unknown; is_system_prefix?: boolean }
        | null
        | undefined;
      const isRendered =
        nodeMeta !== null &&
        nodeMeta !== undefined &&
        "summarized_from" in nodeMeta &&
        nodeMeta.is_system_prefix !== true;

      const extensions: Record<string, unknown> = {};
      if (isRendered && node.toolCalls.length > 0) {
        const ext: Record<string, unknown> = {};
        for (const tc of node.toolCalls) {
          const { title, kind, locations } = toolCallDisplay(tc);
          ext[tc.id] = {
            toolCallId: tc.id,
            title,
            status: "completed",
            locations,
            kind,
            rawInput: tc.arguments,
          };
        }
        extensions["chisel/tool_call_content"] = ext;
      }

      const msg: Record<string, unknown> = {
        ...base,
        tool_calls: node.toolCalls,
        metadata: {
          num_tokens: null,
          is_user_input: null,
          request_id: null,
          metrics: null,
          finish_reason: node.toolCalls.length > 0 ? "tool_calls" : "stop",
          extensions,
          generation_model: generationModel,
          created_at: toIso(node.createdAt),
          telemetry: { source: "assistant", operation: "inference" },
        },
      };

      // Unsigned thinking blocks are dropped: Devin persists provider-sealed
      // signatures and the backend rejects replayed blocks without one.

      return msg;
    }
    case "tool": {
      const toolName = Option.getOrElse(node.toolName, () => "unknown");
      return {
        ...base,
        role: "tool",
        content: node.content,
        tool_call_id: Option.getOrElse(node.toolCallId, () => ""),
        metadata: {
          num_tokens: null,
          is_user_input: null,
          request_id: null,
          metrics: null,
          finish_reason: null,
          extensions: {
            "chisel/tool_result_meta": { success: true, kind: toolName },
          },
          created_at: toIso(node.createdAt),
          telemetry: { source: "tool_result", operation: toolName },
        },
      };
    }
  }
};

const parseToolCalls = (raw: unknown): ReadonlyArray<ToolCall> => {
  if (!Array.isArray(raw)) return [];
  const out: Array<ToolCall> = [];
  for (const tc of raw) {
    if (tc && typeof tc === "object") {
      out.push(
        ToolCall.make({
          id: (tc as any).id ?? "",
          name: (tc as any).name ?? "unknown",
          arguments: (tc as any).arguments ?? {},
          index: (tc as any).index ?? 0,
          kind: (tc as any).kind ?? "function",
        }),
      );
    }
  }
  return out;
};

export const parseChatMessage = (
  chatMessage: unknown,
  rowMetadata: unknown,
  nodeId: number,
  parentNodeId: Option.Option<number>,
  createdAt: number,
): MessageNode => {
  const msg = chatMessage as Record<string, unknown>;
  const role = typeof msg.role === "string" ? (msg.role as MessageNode["role"]) : "system";
  const content = typeof msg.content === "string" ? msg.content : JSON.stringify(msg.content ?? "");
  const toolCalls = parseToolCalls(msg.tool_calls);
  const thinking =
    msg.thinking && typeof (msg.thinking as any).thinking === "string"
      ? Option.some((msg.thinking as any).thinking)
      : Option.none<string>();
  const toolCallId =
    typeof msg.tool_call_id === "string" ? Option.some(msg.tool_call_id) : Option.none<string>();

  let toolName = Option.none<string>();
  const ext = (msg.metadata as any)?.extensions?.["chisel/tool_result_meta"];
  if (ext && typeof ext.kind === "string") {
    toolName = Option.some(ext.kind);
  }

  return MessageNode.make({
    nodeId,
    parentNodeId,
    role,
    content,
    toolCalls,
    toolCallId,
    toolName,
    thinking,
    createdAt,
    metadata: rowMetadata,
  });
};

export const defaultSessionMetadata = () => ({
  total_credit_cost: 0,
  total_acu_cost: 0,
  response_dimensions: [
    {
      uid: "agent_messages",
      group_title: "Response Statistics",
      kind: {
        CumulativeMetric: {
          label: "Agent messages",
          value: 0,
          tail: " message",
          plural_tail: " messages",
          prefix: "",
        },
      },
    },
    {
      uid: "model",
      group_title: "Response Statistics",
      kind: { Metric: { label: "Model", value: "Imported from Cline" } },
    },
  ],
});

export const defaultCogsJson = () =>
  JSON.stringify([
    {
      source: { Session: "User" },
      lifetime: { Unique: "core/plan_mask" },
      set_system_prefix: null,
      append_system_messages: [],
      context: [],
      footer_messages: [],
      user_display: [],
      permissions: [],
      tool_availability: null,
      model: null,
    },
    {
      source: { Session: "User" },
      lifetime: { Unique: "core/accept_edits" },
      set_system_prefix: null,
      append_system_messages: [],
      context: [],
      footer_messages: [],
      user_display: [],
      permissions: [],
      tool_availability: null,
      model: null,
    },
    {
      source: { Session: "System" },
      lifetime: { Unique: "core/parallel-tool-calls" },
      set_system_prefix: null,
      append_system_messages: [],
      context: [],
      footer_messages: [],
      user_display: [],
      permissions: [],
      tool_availability: null,
      model: null,
    },
    {
      source: { Session: "System" },
      lifetime: { Unique: "core/model" },
      set_system_prefix: null,
      append_system_messages: [],
      context: [],
      footer_messages: [],
      user_display: [],
      permissions: [],
      tool_availability: null,
      model: null,
    },
  ]);

/** Columns a real Devin store may leave NULL, with the defaults sepia reads them back as. */
export const sessionFromDevinRow = (
  row: {
    id: string;
    workingDirectory: string;
    backendType: string;
    model: string;
    agentMode: string;
    createdAt: number;
    lastActivityAt: number;
    title: string | null;
    mainChainId: number | null;
    shellLastSeenIndex: number | null;
    cogsJson: string | null;
    workspaceDirs: string | null;
    hidden: number;
    metadata: string | null;
  },
  nodes: ReadonlyArray<MessageNode>,
  promptHistory: ReadonlyArray<PromptHistoryEntry>,
): Session => {
  const parseJson = (s: string | null) => {
    try {
      return JSON.parse(s ?? "{}");
    } catch {
      return {};
    }
  };

  return Session.make({
    id: row.id,
    title: row.title ?? row.id,
    workingDirectory: row.workingDirectory,
    backendType: row.backendType,
    agentMode: row.agentMode,
    model: row.model,
    createdAt: row.createdAt,
    lastActivityAt: row.lastActivityAt,
    mainChainId: row.mainChainId ?? 0,
    shellLastSeenIndex: row.shellLastSeenIndex ?? 0,
    cogsJson: row.cogsJson ?? "[]",
    workspaceDirs: row.workspaceDirs ?? "[]",
    hidden: row.hidden,
    metadata: parseJson(row.metadata),
    nodes,
    promptHistory,
  });
};
