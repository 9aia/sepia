import { expect, test } from "vite-plus/test";
import { Effect, Option, Schema } from "effect";
import { MessageNode, PromptHistoryEntry, Session } from "../src/Domain.js";

const makeSampleSession = (): Session =>
  Session.make({
    id: "test-session",
    title: "Test session",
    workingDirectory: "/tmp/test",
    backendType: "windsurf",
    agentMode: "accept-edits",
    model: "glm-5-2",
    createdAt: 1700000000,
    lastActivityAt: 1700000100,
    mainChainId: 2,
    shellLastSeenIndex: 0,
    cogsJson: "[]",
    workspaceDirs: "[]",
    hidden: 0,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        parentNodeId: Option.none(),
        role: "user",
        content: "hello",
        toolCalls: [],
        createdAt: 1700000000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        parentNodeId: Option.some(0),
        role: "assistant",
        content: "hi",
        toolCalls: [],
        createdAt: 1700000050,
        metadata: { summarized_from: null, is_system_prefix: null },
      }),
    ],
    promptHistory: [
      PromptHistoryEntry.make({ content: "hello", timestamp: 1700000000000, isShell: false }),
    ],
  });

test("Session round-trips through Schema.decodeUnknown", () => {
  const session = makeSampleSession();
  const encoded = Effect.runSync(Schema.encode(Session)(session));
  const decoded = Effect.runSync(Schema.decodeUnknown(Session)(encoded));

  expect(decoded.id).toBe("test-session");
  expect(decoded.title).toBe("Test session");
  expect(decoded.nodes.length).toBe(2);
  expect(decoded.nodes[0].role).toBe("user");
  expect(decoded.nodes[1].role).toBe("assistant");
  expect(decoded.promptHistory[0].content).toBe("hello");
});
