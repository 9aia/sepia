import { Option } from "effect";
import { randomUUID } from "node:crypto";
import {
  MessageNode,
  PromptHistoryEntry,
  Session,
  ToolCall,
  type ToolCallStatus,
  type TokenUsage,
  type ToolResultInfo,
} from "./Domain.js";

const toIso = (ts: number): string => new Date(ts * 1000).toISOString();

const finiteNumber = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

/** ACP tool-call status → the IR's coarser lifecycle. */
export const fromAcpToolCallStatus = (status: unknown): ToolCallStatus | undefined => {
  switch (status) {
    case "completed":
      return "success";
    case "failed":
      return "error";
    case "pending":
    case "in_progress":
      return "pending";
    default:
      return undefined;
  }
};

/** IR status → the ACP status Devin persists in `chisel/tool_call_content`. */
export const toAcpToolCallStatus = (status: ToolCallStatus): string =>
  status === "success" ? "completed" : status === "error" ? "failed" : "pending";

/** Devin `metadata.metrics` keeps the full timing block; only tokens are kept. */
const usageFromMetrics = (metrics: unknown): Option.Option<TokenUsage> => {
  if (metrics === null || typeof metrics !== "object") return Option.none();
  const m = metrics as Record<string, unknown>;
  const input = finiteNumber(m.input_tokens);
  const output = finiteNumber(m.output_tokens);
  if (input === undefined && output === undefined) return Option.none();
  const cacheRead = finiteNumber(m.cache_read_tokens);
  const cacheWrite = finiteNumber(m.cache_creation_tokens);
  return Option.some({
    input: input ?? 0,
    output: output ?? 0,
    ...(cacheRead !== undefined ? { cacheRead } : {}),
    ...(cacheWrite !== undefined ? { cacheWrite } : {}),
  });
};

/** The metrics blob a save writes back: tokens when known, null otherwise. */
const metricsBlob = (node: MessageNode): unknown => {
  const usage = Option.getOrUndefined(node.usage);
  if (usage === undefined) return null;
  return {
    input_tokens: usage.input,
    output_tokens: usage.output,
    cache_read_tokens: usage.cacheRead ?? null,
    cache_creation_tokens: usage.cacheWrite ?? null,
  };
};

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
            status: toAcpToolCallStatus(Option.getOrElse(tc.status, () => "success" as const)),
            locations,
            kind,
            rawInput: tc.arguments,
          };
        }
        extensions["chisel/tool_call_content"] = ext;
      }

      const usage = Option.getOrUndefined(node.usage);
      const msg: Record<string, unknown> = {
        ...base,
        tool_calls: node.toolCalls,
        metadata: {
          num_tokens: usage?.output ?? null,
          is_user_input: null,
          request_id: Option.getOrNull(node.requestId),
          metrics: metricsBlob(node),
          finish_reason: Option.getOrElse(node.finishReason, () =>
            node.toolCalls.length > 0 ? "tool_calls" : "stop",
          ),
          extensions,
          generation_model: Option.getOrElse(node.model, () => generationModel),
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
      const result = Option.getOrUndefined(node.toolResult);
      const extensions: Record<string, unknown> = {
        "chisel/tool_result_meta": {
          success: result === undefined ? true : result.status !== "error",
          kind: toolName,
        },
      };
      if (result?.exitCode !== undefined) {
        extensions["chisel/terminal_output"] = { exit: { exit_code: result.exitCode } };
      }
      if (result?.durationMs !== undefined) {
        extensions["chisel/tool_call_timing"] = { duration_ms: result.durationMs };
      }
      return {
        ...base,
        role: "tool",
        content: node.content,
        tool_call_id: Option.getOrElse(node.toolCallId, () => ""),
        metadata: {
          num_tokens: null,
          is_user_input: null,
          request_id: Option.getOrNull(node.requestId),
          metrics: metricsBlob(node),
          finish_reason: null,
          extensions,
          created_at: toIso(node.createdAt),
          telemetry: { source: "tool_result", operation: toolName },
        },
      };
    }
  }
};

const parseToolCalls = (
  raw: unknown,
  statusById: ReadonlyMap<string, ToolCallStatus>,
): ReadonlyArray<ToolCall> => {
  if (!Array.isArray(raw)) return [];
  const out: Array<ToolCall> = [];
  for (const tc of raw) {
    if (tc && typeof tc === "object") {
      const status = statusById.get((tc as any).id);
      out.push(
        ToolCall.make({
          id: (tc as any).id ?? "",
          name: (tc as any).name ?? "unknown",
          arguments: (tc as any).arguments ?? {},
          index: (tc as any).index ?? 0,
          kind: (tc as any).kind ?? "function",
          ...(status === undefined ? {} : { status: Option.some(status) }),
        }),
      );
    }
  }
  return out;
};

/**
 * Per-call status snapshots from `chisel/tool_call_content` — recorded when
 * the call was issued, so "pending" here often just means "not yet answered";
 * the tool node's `toolResult` and `tool_call_state` carry the outcome.
 */
const toolCallStatusMap = (extensions: unknown): ReadonlyMap<string, ToolCallStatus> => {
  const map = new Map<string, ToolCallStatus>();
  const content = (extensions as Record<string, unknown> | null | undefined)?.[
    "chisel/tool_call_content"
  ];
  if (content === null || typeof content !== "object") return map;
  for (const [id, entry] of Object.entries(content as Record<string, unknown>)) {
    const status = fromAcpToolCallStatus((entry as Record<string, unknown> | null)?.status);
    if (status !== undefined) map.set(id, status);
  }
  return map;
};

/** Outcome fields a `role: "tool"` chat message keeps in its chisel extensions. */
const toolResultFromExtensions = (extensions: unknown): Option.Option<ToolResultInfo> => {
  const ext = extensions as Record<string, Record<string, any>> | null | undefined;
  const resultMeta = ext?.["chisel/tool_result_meta"];
  const success = typeof resultMeta?.success === "boolean" ? resultMeta.success : undefined;
  const exitCode = finiteNumber(ext?.["chisel/terminal_output"]?.exit?.exit_code);
  const durationMs = finiteNumber(ext?.["chisel/tool_call_timing"]?.duration_ms);
  if (success === undefined && exitCode === undefined && durationMs === undefined) {
    return Option.none();
  }
  return Option.some({
    status: success === false ? ("error" as const) : ("success" as const),
    ...(exitCode !== undefined ? { exitCode } : {}),
    ...(durationMs !== undefined ? { durationMs } : {}),
  });
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
  const meta = msg.metadata as Record<string, unknown> | null | undefined;
  const extensions = meta?.extensions;
  const toolCalls = parseToolCalls(msg.tool_calls, toolCallStatusMap(extensions));
  const thinking =
    msg.thinking && typeof (msg.thinking as any).thinking === "string"
      ? Option.some((msg.thinking as any).thinking)
      : Option.none<string>();
  const toolCallId =
    typeof msg.tool_call_id === "string" ? Option.some(msg.tool_call_id) : Option.none<string>();

  let toolName = Option.none<string>();
  const ext = (extensions as Record<string, Record<string, any>> | null | undefined)?.[
    "chisel/tool_result_meta"
  ];
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
    usage: usageFromMetrics(meta?.metrics),
    model:
      typeof meta?.generation_model === "string"
        ? Option.some(meta.generation_model)
        : Option.none<string>(),
    requestId:
      typeof meta?.request_id === "string" ? Option.some(meta.request_id) : Option.none<string>(),
    finishReason:
      typeof meta?.finish_reason === "string"
        ? Option.some(meta.finish_reason)
        : Option.none<string>(),
    toolResult: role === "tool" ? toolResultFromExtensions(extensions) : Option.none(),
    createdAt,
    metadata: rowMetadata,
  });
};

/** The outcome a store recorded for one tool call, folded back onto the call. */
export interface ToolCallOutcome {
  readonly status: ToolCallStatus;
  readonly exitCode?: number;
  readonly durationMs?: number;
}

/** Outcomes keyed by tool-call id, collected from `role: "tool"` nodes. */
export const toolNodeOutcomes = (
  nodes: ReadonlyArray<MessageNode>,
): ReadonlyMap<string, ToolCallOutcome> => {
  const outcomes = new Map<string, ToolCallOutcome>();
  for (const node of nodes) {
    const callId = Option.getOrUndefined(node.toolCallId);
    const result = Option.getOrUndefined(node.toolResult);
    if (node.role !== "tool" || callId === undefined || result === undefined) continue;
    outcomes.set(callId, result);
  }
  return outcomes;
};

const withOutcome = (tc: ToolCall, outcome: ToolCallOutcome): ToolCall =>
  ToolCall.make({
    id: tc.id,
    name: tc.name,
    arguments: tc.arguments,
    index: tc.index,
    kind: tc.kind,
    status: Option.some(outcome.status),
    exitCode: outcome.exitCode === undefined ? tc.exitCode : Option.some(outcome.exitCode),
    durationMs: outcome.durationMs === undefined ? tc.durationMs : Option.some(outcome.durationMs),
  });

/**
 * The result a store keeps on `role: "tool"` messages is the authoritative
 * outcome; fold it onto the `ToolCall` objects that produced it so readers see
 * `call.status` without joining nodes themselves. Calls no result recorded
 * keep whatever `chisel/tool_call_content` said at issue time.
 */
export const applyToolCallOutcomes = (
  nodes: ReadonlyArray<MessageNode>,
  outcomes: ReadonlyMap<string, ToolCallOutcome>,
): ReadonlyArray<MessageNode> => {
  if (outcomes.size === 0) return nodes;
  return nodes.map((node) => {
    if (node.toolCalls.length === 0) return node;
    if (!node.toolCalls.some((tc) => outcomes.has(tc.id))) return node;
    return MessageNode.make({
      nodeId: node.nodeId,
      parentNodeId: node.parentNodeId,
      role: node.role,
      content: node.content,
      toolCalls: node.toolCalls.map((tc) => {
        const outcome = outcomes.get(tc.id);
        return outcome === undefined ? tc : withOutcome(tc, outcome);
      }),
      toolCallId: node.toolCallId,
      toolName: node.toolName,
      thinking: node.thinking,
      usage: node.usage,
      model: node.model,
      requestId: node.requestId,
      finishReason: node.finishReason,
      toolResult: node.toolResult,
      createdAt: node.createdAt,
      metadata: node.metadata,
    });
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
  lineage?: {
    readonly parentSessionId?: string | null;
    readonly agentId?: string | null;
  },
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
    parentSessionId: Option.fromNullable(lineage?.parentSessionId),
    agentId: Option.fromNullable(lineage?.agentId),
    metadata: parseJson(row.metadata),
    nodes,
    promptHistory,
  });
};
