import { Option } from "effect";
import { randomUUID } from "node:crypto";
import {
  MessageNode,
  PromptHistoryEntry,
  Session,
  ToolCall,
  type Block,
  type ToolCallDiff,
  type ToolCallLocation,
  type ToolCallStatus,
  type TokenUsage,
  type ToolResultInfo,
} from "./Domain.js";
import { checkpointsFromMetadata } from "./Shared.js";

const toIso = (ts: number): string => new Date(ts * 1000).toISOString();

const finiteNumber = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

const stringField = (obj: Record<string, unknown>, key: string): string | undefined =>
  typeof obj[key] === "string" ? (obj[key] as string) : undefined;

const present = <K extends string>(key: K, value: string | number | undefined) =>
  value === undefined ? {} : { [key]: value };

/**
 * One entry of `chisel/acp-content-blocks` — the ACP `ContentBlock` the client
 * sent — mapped onto the IR union. `resource`/`resource_link` both land on
 * `file` (embedded vs referenced); kinds the IR can't express are dropped.
 */
const blockFromAcp = (raw: unknown): Block | undefined => {
  if (raw === null || typeof raw !== "object") return undefined;
  const b = raw as Record<string, unknown>;
  switch (b.type) {
    case "text": {
      const text = stringField(b, "text");
      return text === undefined ? undefined : { type: "text", text };
    }
    case "image":
      return {
        type: "image",
        ...present("data", stringField(b, "data")),
        ...present("mimeType", stringField(b, "mimeType")),
        ...present("uri", stringField(b, "uri")),
      };
    case "audio":
      return {
        type: "audio",
        ...present("data", stringField(b, "data")),
        ...present("mimeType", stringField(b, "mimeType")),
      };
    case "resource_link":
      return {
        type: "file",
        ...present("uri", stringField(b, "uri")),
        ...present("name", stringField(b, "name") ?? stringField(b, "title")),
        ...present("mimeType", stringField(b, "mimeType")),
        ...present("size", finiteNumber(b.size)),
      };
    case "resource": {
      const res = b.resource;
      if (res === null || typeof res !== "object") return undefined;
      const r = res as Record<string, unknown>;
      return {
        type: "file",
        ...present("uri", stringField(r, "uri")),
        ...present("mimeType", stringField(r, "mimeType")),
        ...present("text", stringField(r, "text")),
        ...present("data", stringField(r, "blob")),
      };
    }
    default:
      return undefined;
  }
};

/**
 * The block list a chat message records under
 * `metadata.extensions["chisel/acp-content-blocks"]`. Kept only when a
 * non-text block is present — an all-text list duplicates `content` exactly.
 */
export const blocksFromAcp = (raw: unknown): ReadonlyArray<Block> => {
  if (!Array.isArray(raw)) return [];
  const blocks = raw.flatMap((item) => {
    const block = blockFromAcp(item);
    return block === undefined ? [] : [block];
  });
  return blocks.some((block) => block.type !== "text") ? blocks : [];
};

/**
 * An IR block back into the ACP shape the `chisel/acp-content-blocks`
 * extension carries. Embedded files write as `resource`, referenced ones as
 * `resource_link` — matching how the split arrives on read.
 */
const blockToAcp = (block: Block): unknown => {
  switch (block.type) {
    case "text":
      return { type: "text", text: block.text };
    case "image":
      return {
        type: "image",
        ...present("data", block.data),
        ...present("mimeType", block.mimeType),
        ...present("uri", block.uri),
      };
    case "audio":
      return {
        type: "audio",
        ...present("data", block.data),
        ...present("mimeType", block.mimeType),
      };
    case "file": {
      if (block.text !== undefined || block.data !== undefined) {
        // ACP `resource` (an embedded payload) has no name slot — a named
        // attachment keeps its name only while it stays a `resource_link`.
        return {
          type: "resource",
          resource: {
            uri: block.uri ?? "",
            ...present("mimeType", block.mimeType),
            ...(block.text !== undefined ? { text: block.text } : { blob: block.data }),
          },
        };
      }
      return {
        type: "resource_link",
        uri: block.uri ?? block.name ?? "",
        name: block.name ?? block.uri ?? "",
        ...present("mimeType", block.mimeType),
        ...present("size", block.size),
      };
    }
  }
};

/** The extension entry a saved chat_message gets when the node carries blocks. */
const contentBlocksExtension = (node: MessageNode): Record<string, unknown> =>
  node.blocks.length === 0 ? {} : { "chisel/acp-content-blocks": node.blocks.map(blockToAcp) };

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
          extensions: contentBlocksExtension(node),
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

      const extensions: Record<string, unknown> = contentBlocksExtension(node);
      if (isRendered && node.toolCalls.length > 0) {
        const ext: Record<string, unknown> = {};
        for (const tc of node.toolCalls) {
          const { title, kind, locations } = toolCallDisplay(tc);
          ext[tc.id] = {
            toolCallId: tc.id,
            title,
            status: toAcpToolCallStatus(Option.getOrElse(tc.status, () => "success" as const)),
            // The locations the call recorded win; the arg-derived path is
            // only a stand-in for calls a foreign store couldn't locate.
            locations: tc.locations.length > 0 ? tc.locations : locations,
            kind,
            rawInput: tc.arguments,
            ...(tc.diffs.length === 0
              ? {}
              : {
                  content: tc.diffs.map((diff) => ({
                    type: "diff",
                    path: diff.path,
                    ...(diff.oldText === undefined ? {} : { oldText: diff.oldText }),
                    ...(diff.newText === undefined ? {} : { newText: diff.newText }),
                  })),
                }),
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

      // The `thinking` object is written only with its provider seal:
      // unsigned thinking is dropped — the backend rejects replayed blocks
      // without a signature.
      const signature = Option.getOrUndefined(node.thinkingSignature);
      if (signature !== undefined) {
        msg.thinking = {
          thinking: Option.getOrElse(node.thinking, () => ""),
          signature,
        };
      }

      return msg;
    }
    case "tool": {
      const toolName = Option.getOrElse(node.toolName, () => "unknown");
      const result = Option.getOrUndefined(node.toolResult);
      const extensions: Record<string, unknown> = {
        ...contentBlocksExtension(node),
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

/** `locations` entries — `{path, line?}` objects with a readable path. */
const locationsFromAcp = (raw: unknown): ReadonlyArray<ToolCallLocation> => {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item) => {
    if (item === null || typeof item !== "object") return [];
    const loc = item as Record<string, unknown>;
    const path = stringField(loc, "path");
    if (path === undefined) return [];
    const line = finiteNumber(loc.line);
    return [{ path, ...(line === undefined ? {} : { line }) }];
  });
};

/**
 * `diff` entries of a tool call's `content` — `{type:"diff", path, oldText?,
 * newText?}`. Other content kinds (terminal output, text) are not file
 * changes and stay in the tool result.
 */
const diffsFromAcpContent = (raw: unknown): ReadonlyArray<ToolCallDiff> => {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item) => {
    if (item === null || typeof item !== "object") return [];
    const c = item as Record<string, unknown>;
    if (c.type !== "diff") return [];
    const path = stringField(c, "path");
    if (path === undefined) return [];
    const oldText = stringField(c, "oldText");
    const newText = stringField(c, "newText");
    return [
      {
        path,
        ...(oldText === undefined ? {} : { oldText }),
        ...(newText === undefined ? {} : { newText }),
      },
    ];
  });
};

/**
 * Sepia-written tool_calls keep `{path, oldText?, newText?}` on the call's
 * `diffs` field — no `type` marker there, the field itself says what it is.
 */
const diffsFromEntry = (raw: unknown): ReadonlyArray<ToolCallDiff> => {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item) => {
    if (item === null || typeof item !== "object") return [];
    const c = item as Record<string, unknown>;
    const path = stringField(c, "path");
    if (path === undefined) return [];
    const oldText = stringField(c, "oldText");
    const newText = stringField(c, "newText");
    return [
      {
        path,
        ...(oldText === undefined ? {} : { oldText }),
        ...(newText === undefined ? {} : { newText }),
      },
    ];
  });
};

/**
 * What `chisel/tool_call_content` records per call beyond its status —
 * the ACP snapshot taken when the call was issued.
 */
interface ToolCallExt {
  readonly status?: ToolCallStatus;
  readonly locations?: ReadonlyArray<ToolCallLocation>;
  readonly diffs?: ReadonlyArray<ToolCallDiff>;
}

const parseToolCalls = (
  raw: unknown,
  extById: ReadonlyMap<string, ToolCallExt>,
): ReadonlyArray<ToolCall> => {
  if (!Array.isArray(raw)) return [];
  const out: Array<ToolCall> = [];
  for (const tc of raw) {
    if (tc && typeof tc === "object") {
      const entry = tc as Record<string, unknown>;
      const ext = extById.get(entry.id as string);
      // Sepia-written tool_calls keep locations/diffs on the call itself;
      // the chisel extension is the store-native carrier and wins.
      const locations = ext?.locations ?? locationsFromAcp(entry.locations);
      const diffs = ext?.diffs ?? diffsFromEntry(entry.diffs);
      out.push(
        ToolCall.make({
          id: (entry.id as string) ?? "",
          name: (entry.name as string) ?? "unknown",
          arguments: entry.arguments ?? {},
          index: (entry.index as number) ?? 0,
          kind: (entry.kind as string) ?? "function",
          ...(ext?.status === undefined ? {} : { status: Option.some(ext.status) }),
          locations,
          diffs,
        }),
      );
    }
  }
  return out;
};

/**
 * Per-call snapshots from `chisel/tool_call_content` — the ACP `ToolCall`
 * recorded when the call was issued, so "pending" here often just means
 * "not yet answered"; the tool node's `toolResult` and `tool_call_state`
 * carry the outcome. `locations` and `content` diffs survive verbatim.
 */
const toolCallExtMap = (extensions: unknown): ReadonlyMap<string, ToolCallExt> => {
  const map = new Map<string, ToolCallExt>();
  const content = (extensions as Record<string, unknown> | null | undefined)?.[
    "chisel/tool_call_content"
  ];
  if (content === null || typeof content !== "object") return map;
  for (const [id, entry] of Object.entries(content as Record<string, unknown>)) {
    if (entry === null || typeof entry !== "object") continue;
    const e = entry as Record<string, unknown>;
    const status = fromAcpToolCallStatus(e.status);
    const locations = locationsFromAcp(e.locations);
    const diffs = diffsFromAcpContent(e.content);
    map.set(id, {
      ...(status === undefined ? {} : { status }),
      ...(locations.length === 0 ? {} : { locations }),
      ...(diffs.length === 0 ? {} : { diffs }),
    });
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
  const blocks = blocksFromAcp(
    (extensions as Record<string, unknown> | null | undefined)?.["chisel/acp-content-blocks"],
  );
  const toolCalls = parseToolCalls(msg.tool_calls, toolCallExtMap(extensions));
  const thinkingRaw = msg.thinking;
  const thinking =
    thinkingRaw && typeof (thinkingRaw as any).thinking === "string"
      ? Option.some((thinkingRaw as any).thinking)
      : Option.none<string>();
  // `signature` is the provider's seal on the thinking text — opaque, kept
  // verbatim (an empty string means Devin stored the block unsigned).
  const thinkingSignature =
    thinkingRaw && typeof (thinkingRaw as any).signature === "string"
      ? Option.some((thinkingRaw as any).signature)
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
    blocks,
    toolCalls,
    toolCallId,
    toolName,
    thinking,
    thinkingSignature,
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
    checkpoints: checkpointsFromMetadata(parseJson(row.metadata)),
    metadata: parseJson(row.metadata),
    nodes,
    promptHistory,
  });
};

// Canonical shared services — re-exported so `Devin.X` consumers keep working.
export {
  applyToolCallOutcomes,
  checkpointsFromMetadata,
  defaultCogsJson,
  defaultSessionMetadata,
  SESSION_CHECKPOINTS_KEY,
  type ToolCallOutcome,
  toolNodeOutcomes,
} from "./Shared.js";
