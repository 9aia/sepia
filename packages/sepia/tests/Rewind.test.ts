import { Option } from "effect";
import { expect, test } from "vite-plus/test";
import { MessageNode, Session, ToolCall } from "../src/Domain.js";
import * as Rewind from "../src/Rewind.js";

const node = (
  nodeId: number,
  role: MessageNode["role"],
  input: {
    toolCalls?: ReadonlyArray<ToolCall>;
    toolCallId?: string;
    createdAt?: number;
    metadata?: unknown;
  } = {},
): MessageNode =>
  MessageNode.make({
    nodeId,
    parentNodeId: nodeId === 0 ? Option.none() : Option.some(nodeId - 1),
    role,
    content: `${role} ${nodeId}`,
    toolCalls: input.toolCalls ?? [],
    toolCallId: Option.fromNullable(input.toolCallId),
    createdAt: input.createdAt ?? 1700000000 + nodeId,
    metadata: input.metadata ?? null,
  });

const session = (
  nodes: ReadonlyArray<MessageNode>,
  checkpoints: Session["checkpoints"] = [],
): Session =>
  Session.make({
    id: "s1",
    title: "Session",
    workingDirectory: "/work",
    backendType: "windsurf",
    model: "test-model",
    createdAt: 1700000000,
    lastActivityAt: 1700000000 + nodes.length,
    mainChainId: nodes.length - 1,
    checkpoints,
    metadata: null,
    nodes,
  });

const chatSession = (): Session =>
  session([
    node(0, "system"),
    node(1, "user"),
    node(2, "assistant"),
    node(3, "tool", { toolCallId: "call_1" }),
    node(4, "user"),
    node(5, "assistant", {
      toolCalls: [ToolCall.make({ id: "call_2", name: "edit", arguments: {} })],
    }),
    node(6, "tool", { toolCallId: "call_2" }),
    node(7, "user"),
    node(8, "assistant"),
  ]);

test("planRewind requires exactly one selector", () => {
  const s = chatSession();
  expect(Rewind.planRewind(s, {}).ok).toBe(false);
  expect(Rewind.planRewind(s, { nodeId: 2, turns: 1 }).ok).toBe(false);
  expect(Rewind.planRewind(s, { turns: 1, checkpoint: "abc" }).ok).toBe(false);
});

test("planRewind nodeId keeps through the named node and drops the tail", () => {
  const s = chatSession();
  const result = Rewind.planRewind(s, { nodeId: 4 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.keepCount).toBe(5);
  expect(result.plan.kept.map((n) => n.nodeId)).toEqual([0, 1, 2, 3, 4]);
  expect(result.plan.removed.map((n) => n.nodeId)).toEqual([5, 6, 7, 8]);
  // call_2 dies with its turn; call_1 survives because node 3 stays.
  expect(result.plan.removedToolCallIds).toEqual(["call_2"]);
});

test("planRewind nodeId on the tail is a no-op", () => {
  const s = chatSession();
  const result = Rewind.planRewind(s, { nodeId: 8 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.removed).toEqual([]);
});

test("planRewind rejects an unknown nodeId", () => {
  const result = Rewind.planRewind(chatSession(), { nodeId: 42 });
  expect(result.ok).toBe(false);
  if (!result.ok) expect(result.reason).toContain("Unknown node: 42");
});

test("planRewind turns:1 drops the last user turn including the prompt", () => {
  const result = Rewind.planRewind(chatSession(), { turns: 1 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.kept.map((n) => n.nodeId)).toEqual([0, 1, 2, 3, 4, 5, 6]);
});

test("planRewind turns:2 drops the last two user turns", () => {
  const result = Rewind.planRewind(chatSession(), { turns: 2 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.kept.map((n) => n.nodeId)).toEqual([0, 1, 2, 3]);
});

test("planRewind turns past the start rewinds to before the first turn", () => {
  const result = Rewind.planRewind(chatSession(), { turns: 9 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  // Everything before the first user node — the system prefix survives.
  expect(result.plan.kept.map((n) => n.nodeId)).toEqual([0]);
});

test("planRewind turns validates the count and needs a user node", () => {
  expect(Rewind.planRewind(chatSession(), { turns: 0 }).ok).toBe(false);
  expect(Rewind.planRewind(chatSession(), { turns: 1.5 }).ok).toBe(false);
  const noUsers = session([node(0, "system"), node(1, "assistant")]);
  expect(Rewind.planRewind(noUsers, { turns: 1 }).ok).toBe(false);
});

test("planRewind checkpoint resolves to the last node at or before its time", () => {
  const s = session(
    [
      node(0, "user", { createdAt: 1700000000 }),
      node(1, "assistant", { createdAt: 1700000005 }),
      node(2, "assistant", { createdAt: 1700000010 }),
    ],
    [{ ref: "sha-1", createdAt: 1700000005500 }],
  );
  const result = Rewind.planRewind(s, { checkpoint: "sha-1" });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.kept.map((n) => n.nodeId)).toEqual([0, 1]);
  expect(result.plan.removed.map((n) => n.nodeId)).toEqual([2]);
});

test("planRewind checkpoint rejects an unknown ref", () => {
  const s = session([node(0, "user")], [{ ref: "sha-1", createdAt: 1 }]);
  const result = Rewind.planRewind(s, { checkpoint: "sha-9" });
  expect(result.ok).toBe(false);
  if (!result.ok) expect(result.reason).toContain("Unknown checkpoint ref");
});

test("removedToolCallIds excludes calls a kept node still claims", () => {
  const s = session([
    node(0, "assistant", {
      toolCalls: [ToolCall.make({ id: "shared", name: "edit", arguments: {} })],
    }),
    node(1, "tool", { toolCallId: "shared" }),
    node(2, "assistant", {
      toolCalls: [ToolCall.make({ id: "gone", name: "edit", arguments: {} })],
    }),
  ]);
  // Keep node 0 — node 1's tool result node is removed but its call id
  // is still claimed by the kept assistant turn, so it stays.
  const result = Rewind.planRewind(s, { nodeId: 0 });
  expect(result.ok).toBe(true);
  if (!result.ok) return;
  expect(result.plan.removedToolCallIds).toEqual(["gone"]);
});

test("rewindSession applies the cut and moves the tail markers back", () => {
  const s = chatSession();
  const planned = Rewind.planRewind(s, { nodeId: 4 });
  expect(planned.ok).toBe(true);
  if (!planned.ok) return;
  const truncated = Rewind.rewindSession(s, planned.plan);
  expect(truncated.nodes.map((n) => n.nodeId)).toEqual([0, 1, 2, 3, 4]);
  expect(truncated.lastActivityAt).toBe(s.nodes[4]!.createdAt);
  expect(truncated.mainChainId).toBe(4);
  expect(truncated.id).toBe(s.id);
  expect(truncated.title).toBe(s.title);
});

test("rewindSession to empty keeps the session's creation stamp", () => {
  const s = chatSession();
  const truncated = Rewind.rewindSession(s, {
    keepCount: 0,
    kept: [],
    removed: s.nodes,
    removedToolCallIds: [],
  });
  expect(truncated.nodes).toEqual([]);
  expect(truncated.lastActivityAt).toBe(s.createdAt);
  expect(truncated.mainChainId).toBe(0);
});
