/**
 * Shared domain services every adapter may use — outcome folding, canonical
 * session defaults, checkpoint-metadata convention, project-dir slugs.
 * Lives in core so no adapter imports another adapter.
 */
import { Option } from "effect";

import { type CheckpointRef, MessageNode, ToolCall, type ToolCallStatus } from "./Domain.js";

const finiteNumber = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

const stringField = (obj: Record<string, unknown>, key: string): string | undefined =>
  typeof obj[key] === "string" ? (obj[key] as string) : undefined;

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
    locations: tc.locations,
    diffs: tc.diffs,
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
      blocks: node.blocks,
      toolCalls: node.toolCalls.map((tc) => {
        const outcome = outcomes.get(tc.id);
        return outcome === undefined ? tc : withOutcome(tc, outcome);
      }),
      toolCallId: node.toolCallId,
      toolName: node.toolName,
      thinking: node.thinking,
      thinkingSignature: node.thinkingSignature,
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

/**
 * The namespaced slot sepia keeps workspace-snapshot refs under in the
 * `sessions.metadata` JSON — Devin's own metadata has no checkpoint slot,
 * and a `sepia/`-prefixed key can't collide with its extension names.
 */
export const SESSION_CHECKPOINTS_KEY = "sepia/checkpoints";

/**
 * Tolerant read of a `sepia/checkpoints` metadata value: entries that are
 * not `{ref, createdAt}` objects are dropped rather than failing the row.
 */
export const checkpointsFromMetadata = (metadata: unknown): ReadonlyArray<CheckpointRef> => {
  if (metadata === null || typeof metadata !== "object") return [];
  const raw = (metadata as Record<string, unknown>)[SESSION_CHECKPOINTS_KEY];
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item) => {
    if (item === null || typeof item !== "object") return [];
    const c = item as Record<string, unknown>;
    const ref = stringField(c, "ref");
    const createdAt = finiteNumber(c.createdAt);
    if (ref === undefined || createdAt === undefined) return [];
    const runCount = finiteNumber(c.runCount);
    const kind = stringField(c, "kind");
    return [
      {
        ref,
        createdAt,
        ...(runCount === undefined ? {} : { runCount }),
        ...(kind === undefined ? {} : { kind }),
      },
    ];
  });
};

/** Columns a real Devin store may leave NULL, with the defaults sepia reads them back as. */

/**
 * The project dir name is the cwd with non-alphanumerics flattened to `-`,
 * so decoding is lossy — `-home-me-proj` → `/home/me/proj` recovers the
 * common case. Used only when no entry in the file carries a `cwd`.
 */
export const decodeProjectDir = (name: string): string => {
  const decoded = name.replaceAll("-", "/");
  return decoded.startsWith("/") ? decoded : `/${decoded}`;
};

/**
 * The inverse of `decodeProjectDir` — the dir name a transcript for `cwd`
 * lands under (`/home/me/proj` → `-home-me-proj`). Lossy the same way:
 * `my proj` and `my-proj` collide.
 */
export const encodeProjectDir = (cwd: string): string => cwd.replace(/[^a-zA-Z0-9]/g, "-");
