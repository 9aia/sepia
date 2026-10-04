import { describe, expect, it } from "vite-plus/test";
import { Option } from "effect";
import {
  sessionFromHistory,
  sessionFromJson,
  sessionToJson,
  type ImportedHistoryMessage,
} from "../src/Conversion.js";
import { MessageNode, Session } from "../src/Domain.js";

describe("session wire format", () => {
  it("round-trips a node's blocks through sessionToJson/sessionFromJson", () => {
    const session = Session.make({
      id: "blocks",
      title: "with attachments",
      workingDirectory: "/work",
      model: "m",
      createdAt: 1,
      lastActivityAt: 2,
      mainChainId: 1,
      metadata: null,
      nodes: [
        MessageNode.make({
          nodeId: 0,
          role: "user",
          content: "see attached",
          blocks: [
            { type: "text", text: "see attached" },
            { type: "image", data: "aGk=", mimeType: "image/png" },
            { type: "file", uri: "file:///work/a.ts", name: "a.ts", size: 9 },
          ],
          createdAt: 1,
          metadata: null,
        }),
        MessageNode.make({
          nodeId: 1,
          parentNodeId: Option.some(0),
          role: "assistant",
          content: "done",
          createdAt: 2,
          metadata: null,
        }),
      ],
    });

    const json = sessionToJson(session);
    const nodes = json.nodes ?? [];
    expect(nodes[0]?.blocks).toEqual([
      { type: "text", text: "see attached" },
      { type: "image", data: "aGk=", mimeType: "image/png" },
      { type: "file", uri: "file:///work/a.ts", name: "a.ts", size: 9 },
    ]);
    // A block-less node encodes the default empty list, which decodes back.
    expect(nodes[1]?.blocks).toEqual([]);

    const decoded = sessionFromJson(json);
    expect(decoded.nodes[0]?.blocks).toEqual(nodes[0]?.blocks);
    expect(decoded.nodes[1]?.blocks).toEqual([]);
  });
});

describe("sessionFromHistory", () => {
  const base = {
    id: "h1",
    title: "history",
    cwd: "/work",
    model: "m",
  };

  it("carries history-item blocks onto the rebuilt node", () => {
    const history: ReadonlyArray<ImportedHistoryMessage> = [
      {
        role: "user",
        content: "look",
        createdAt: 1_700_000_000_000,
        blocks: [
          { type: "text", text: "look" },
          { type: "image", uri: "https://x/y.png" },
        ],
      },
      { role: "assistant", content: "done", createdAt: 1_700_000_001_000 },
    ];

    const session = sessionFromHistory({ ...base, history });
    expect(session.nodes[0]?.blocks).toEqual([
      { type: "text", text: "look" },
      { type: "image", uri: "https://x/y.png" },
    ]);
    // No blocks on the wire → the node keeps none.
    expect(session.nodes[1]?.blocks).toEqual([]);
  });
});
