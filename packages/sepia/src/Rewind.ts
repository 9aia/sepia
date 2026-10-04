import { Option } from "effect";
import { MessageNode, Session } from "./Domain.js";

/**
 * Conversation rewind planning — pure math over `Session.nodes`, separated
 * from the store-specific writers (Devin `message_nodes` row delete, Cline
 * messages-array slice, Claude JSONL truncation, Cursor checkpoint
 * re-root) so the same cut is unit testable and every store truncates
 * identically.
 *
 * A rewind is a prefix cut: `nodes[0..keepCount)` survive, everything
 * after is gone — "the conversation ends at this message". Branching
 * detail: nodes are the store's emission order, so the cut drops every
 * node recorded after the target — later side branches included. How a
 * store persists the cut (in-place delete vs truncating rewrite vs DAG
 * re-root) is the writer's call; this file only computes it.
 */

/** Selector for the cut point — exactly one member is set. */
export interface RewindTarget {
  /** Keep this node and everything before it. */
  readonly nodeId?: number;
  /** Drop the last N user turns (a user node plus everything up to the next). */
  readonly turns?: number;
  /** Keep up to the last node recorded at or before this checkpoint ref. */
  readonly checkpoint?: string;
}

export interface RewindPlan {
  /** `nodes[0..keepCount)` survive. */
  readonly keepCount: number;
  readonly kept: ReadonlyArray<MessageNode>;
  readonly removed: ReadonlyArray<MessageNode>;
  /**
   * Tool-call ids that exist only inside removed nodes — the rows a
   * `tool_call_state`-style store cleans alongside the messages. A call a
   * kept assistant turn also claims is excluded so its state survives.
   */
  readonly removedToolCallIds: ReadonlyArray<string>;
}

export type RewindPlanResult =
  | { readonly ok: true; readonly plan: RewindPlan }
  | { readonly ok: false; readonly reason: string };

const fail = (reason: string): RewindPlanResult => ({ ok: false, reason });

/**
 * Resolve a `RewindTarget` to a prefix cut of `session.nodes`.
 *
 * - `nodeId`: keep through the node with that id (`Unknown node` when it
 *   is not in the session).
 * - `turns: N`: keep everything before the N-th-from-last `user` node —
 *   the user message itself and the assistant's response both go. Fewer
 *   than N user nodes rewinds to before the first turn.
 * - `checkpoint`: resolve the recorded `Session.checkpoints` ref to the
 *   last node at or before its `createdAt` (epoch ms; node `createdAt`
 *   is epoch s). Approximate by design — the transcript position the
 *   checkpoint's turn ended at.
 *
 * `removed` is empty when the target is already the tail — callers treat
 * that as a no-op rather than writing the store.
 */
export const planRewind = (session: Session, target: RewindTarget): RewindPlanResult => {
  const nodes = session.nodes;
  const selected =
    (target.nodeId === undefined ? 0 : 1) +
    (target.turns === undefined ? 0 : 1) +
    (target.checkpoint === undefined ? 0 : 1);
  if (selected !== 1) {
    return fail("rewind needs exactly one of nodeId, turns or checkpoint");
  }

  let keepCount: number;
  if (target.nodeId !== undefined) {
    const index = nodes.findIndex((node) => node.nodeId === target.nodeId);
    if (index === -1) return fail(`Unknown node: ${target.nodeId}`);
    keepCount = index + 1;
  } else if (target.turns !== undefined) {
    if (!Number.isInteger(target.turns) || target.turns < 1) {
      return fail("turns must be a positive integer");
    }
    const userIndices = nodes.flatMap((node, index) => (node.role === "user" ? [index] : []));
    if (userIndices.length === 0) {
      return fail("the session has no user turns to drop");
    }
    const fromEnd = userIndices.length - target.turns;
    keepCount = userIndices[Math.max(0, fromEnd)] ?? 0;
  } else {
    const entry = session.checkpoints.find((candidate) => candidate.ref === target.checkpoint);
    if (entry === undefined) {
      return fail(`Unknown checkpoint ref: ${target.checkpoint}`);
    }
    keepCount = 0;
    for (let index = 0; index < nodes.length; index++) {
      if ((nodes[index]?.createdAt ?? 0) * 1000 <= entry.createdAt) keepCount = index + 1;
    }
  }

  const kept = nodes.slice(0, keepCount);
  const removed = nodes.slice(keepCount);
  const removedIds = new Set<string>();
  for (const node of removed) {
    for (const call of node.toolCalls) removedIds.add(call.id);
    const answered = Option.getOrUndefined(node.toolCallId);
    if (answered !== undefined) removedIds.add(answered);
  }
  const survivingIds = new Set<string>();
  for (const node of kept) {
    for (const call of node.toolCalls) survivingIds.add(call.id);
  }
  return {
    ok: true,
    plan: {
      keepCount,
      kept,
      removed,
      removedToolCallIds: [...removedIds].filter((id) => !survivingIds.has(id)),
    },
  };
};

/**
 * `session` with the plan's cut applied — the form save-based writers
 * (Cline manifest+messages rebuild, Cursor checkpoint re-root) persist.
 * `lastActivityAt`/`mainChainId` move back to the surviving tail.
 */
export const rewindSession = (session: Session, plan: RewindPlan): Session => {
  const tail = plan.kept[plan.kept.length - 1];
  return Session.make({
    id: session.id,
    title: session.title,
    workingDirectory: session.workingDirectory,
    backendType: session.backendType,
    agentMode: session.agentMode,
    model: session.model,
    createdAt: session.createdAt,
    lastActivityAt: tail?.createdAt ?? session.createdAt,
    mainChainId: tail?.nodeId ?? 0,
    shellLastSeenIndex: session.shellLastSeenIndex,
    cogsJson: session.cogsJson,
    workspaceDirs: session.workspaceDirs,
    hidden: session.hidden,
    parentSessionId: session.parentSessionId,
    agentId: session.agentId,
    checkpoints: session.checkpoints,
    metadata: session.metadata,
    nodes: plan.kept,
    promptHistory: session.promptHistory,
  });
};
