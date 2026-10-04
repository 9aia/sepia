import { expect, test } from "vite-plus/test";
import { Effect, Option, Schema } from "effect";
import { MessageNode, PromptHistoryEntry, Session, ToolCall } from "../src/Domain.js";

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

test("MessageNode blocks decode absent, encode and round-trip", () => {
  const withBlocks = MessageNode.make({
    nodeId: 0,
    role: "user",
    content: "look at this",
    blocks: [
      { type: "text", text: "look at this" },
      { type: "image", data: "aGk=", mimeType: "image/png" },
      { type: "file", uri: "file:///work/a.ts", name: "a.ts", size: 12 },
      { type: "audio", data: "AAE=", mimeType: "audio/wav" },
    ],
    createdAt: 1700000000,
    metadata: null,
  });
  const encoded = Effect.runSync(Schema.encode(MessageNode)(withBlocks));
  const decoded = Effect.runSync(Schema.decodeUnknown(MessageNode)(encoded));
  expect(decoded.blocks).toEqual(withBlocks.blocks);

  // A block-less node encodes no `blocks` key and decodes to an empty list.
  const plain = MessageNode.make({
    nodeId: 1,
    role: "assistant",
    content: "hi",
    createdAt: 1700000001,
    metadata: null,
  });
  const plainEncoded = Effect.runSync(Schema.encode(MessageNode)(plain)) as Record<string, unknown>;
  expect(plainEncoded.blocks).toEqual([]);
  expect(Effect.runSync(Schema.decodeUnknown(MessageNode)(plainEncoded)).blocks).toEqual([]);
});

test("ToolCall locations/diffs and Session checkpoints round-trip; absent decodes empty", () => {
  const call = ToolCall.make({
    id: "c1",
    name: "edit",
    arguments: { file_path: "/a.ts" },
    locations: [{ path: "/a.ts", line: 12 }],
    diffs: [
      { path: "/a.ts", oldText: "old", newText: "new" },
      { path: "/b.ts", newText: "body" },
    ],
  });
  const decodedCall = Effect.runSync(
    Schema.decodeUnknown(ToolCall)(Effect.runSync(Schema.encode(ToolCall)(call))),
  );
  expect(decodedCall.locations).toEqual([{ path: "/a.ts", line: 12 }]);
  expect(decodedCall.diffs).toEqual([
    { path: "/a.ts", oldText: "old", newText: "new" },
    { path: "/b.ts", newText: "body" },
  ]);

  // Older stores carry no such keys — decode fills the defaults.
  const bare = Effect.runSync(
    Schema.decodeUnknown(ToolCall)({ id: "c2", name: "read", arguments: {} }),
  );
  expect(bare.locations).toEqual([]);
  expect(bare.diffs).toEqual([]);

  const session = Session.make({
    id: "s",
    title: "s",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: 0,
    checkpoints: [{ ref: "abc", createdAt: 100, runCount: 3, kind: "stash" }],
    metadata: null,
  });
  const decoded = Effect.runSync(
    Schema.decodeUnknown(Session)(Effect.runSync(Schema.encode(Session)(session))),
  );
  expect(decoded.checkpoints).toEqual([{ ref: "abc", createdAt: 100, runCount: 3, kind: "stash" }]);
});
