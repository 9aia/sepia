import { describe, expect, it } from "vite-plus/test";
import { Option } from "effect";
import {
  buildChatMessage,
  defaultCogsJson,
  defaultSessionMetadata,
  parseChatMessage,
  sessionFromDevinRow,
} from "../src/Devin.js";
import { MessageNode, ToolCall } from "../src/Domain.js";

const node = (overrides: Partial<Parameters<typeof MessageNode.make>[0]> = {}): MessageNode =>
  MessageNode.make({
    nodeId: 1,
    parentNodeId: Option.none(),
    role: "assistant",
    content: "text",
    toolCalls: [],
    toolCallId: Option.none(),
    toolName: Option.none(),
    thinking: Option.none(),
    createdAt: 1_700_000_000,
    metadata: null,
    ...overrides,
  });

describe("buildChatMessage", () => {
  it("system and user roles carry minimal metadata", () => {
    const sys = buildChatMessage(node({ role: "system" }), "m") as Record<string, unknown>;
    expect(sys).toMatchObject({ role: "system", content: "text" });
    expect(sys.metadata).toBeUndefined();

    const user = buildChatMessage(node({ role: "user" }), "m") as Record<string, unknown>;
    const meta = user.metadata as Record<string, unknown>;
    expect(meta.is_user_input).toBe(true);
    expect(meta.created_at).toBe(new Date(1_700_000_000_000).toISOString());
    expect(meta.telemetry).toEqual({ source: "user", operation: "input" });
  });

  it("an assistant message with no tool calls finishes with 'stop'", () => {
    const msg = buildChatMessage(node({ role: "assistant" }), "devin-model") as Record<
      string,
      unknown
    >;
    const meta = msg.metadata as Record<string, unknown>;
    expect(meta.finish_reason).toBe("stop");
    expect(meta.generation_model).toBe("devin-model");
    expect(meta.extensions).toEqual({});
  });

  it("rendered tool calls land in the chisel extension with display metadata", () => {
    const calls = [
      ToolCall.make({ id: "r1", name: "read", arguments: { file_path: "/a.ts" } }),
      ToolCall.make({ id: "x1", name: "exec", arguments: {} }),
      ToolCall.make({ id: "g1", name: "grep", arguments: {} }),
      ToolCall.make({ id: "w1", name: "webfetch", arguments: {} }),
      ToolCall.make({ id: "e1", name: "edit", arguments: { file_path: "/b.ts" } }),
      ToolCall.make({ id: "w2", name: "write", arguments: {} }),
      ToolCall.make({ id: "z1", name: "zzz", arguments: {} }),
    ];
    const msg = buildChatMessage(
      node({ role: "assistant", toolCalls: calls, metadata: { summarized_from: 3 } }),
      "m",
    ) as Record<string, unknown>;
    const meta = msg.metadata as {
      finish_reason: string;
      extensions: {
        "chisel/tool_call_content": Record<
          string,
          {
            title: string;
            kind: string;
            locations: Array<{ path: string }>;
          }
        >;
      };
    };
    expect(meta.finish_reason).toBe("tool_calls");
    const ext = meta.extensions["chisel/tool_call_content"];
    expect(ext.r1).toMatchObject({
      title: "Read file",
      kind: "read",
      locations: [{ path: "/a.ts" }],
    });
    expect(ext.x1).toMatchObject({ title: "Ran command", kind: "execute" });
    expect(ext.g1).toMatchObject({ title: "Searched codebase", kind: "search" });
    expect(ext.w1).toMatchObject({ title: "Fetched web content", kind: "fetch" });
    expect(ext.e1).toMatchObject({
      title: "Edited file",
      kind: "edit",
      locations: [{ path: "/b.ts" }],
    });
    expect(ext.w2).toMatchObject({ title: "Wrote file", kind: "edit" });
    expect(ext.z1).toMatchObject({ title: "zzz", kind: "function" });
  });

  it("skips the chisel extension for non-rendered or is_system_prefix messages", () => {
    const noRender = buildChatMessage(
      node({
        role: "assistant",
        toolCalls: [ToolCall.make({ id: "t", name: "read", arguments: {} })],
      }),
      "m",
    ) as { metadata: { extensions: Record<string, unknown> } };
    expect(noRender.metadata.extensions).toEqual({});

    const prefix = buildChatMessage(
      node({
        role: "assistant",
        toolCalls: [ToolCall.make({ id: "t", name: "read", arguments: {} })],
        metadata: { summarized_from: 1, is_system_prefix: true },
      }),
      "m",
    ) as { metadata: { extensions: Record<string, unknown> } };
    expect(prefix.metadata.extensions).toEqual({});
  });

  it("tool results carry the tool name and call id", () => {
    const msg = buildChatMessage(
      node({
        role: "tool",
        toolName: Option.some("exec"),
        toolCallId: Option.some("call-9"),
      }),
      "m",
    ) as Record<string, unknown>;
    expect(msg.tool_call_id).toBe("call-9");
    const meta = msg.metadata as Record<string, unknown>;
    expect(meta.telemetry).toEqual({ source: "tool_result", operation: "exec" });
    expect(meta.extensions).toEqual({
      "chisel/tool_result_meta": { success: true, kind: "exec" },
    });
  });

  it("tool results fall back to 'unknown' and empty id", () => {
    const msg = buildChatMessage(node({ role: "tool" }), "m") as Record<string, unknown>;
    expect(msg.tool_call_id).toBe("");
    const meta = msg.metadata as { extensions: Record<string, { kind: string }> };
    expect(meta.extensions["chisel/tool_result_meta"]?.kind).toBe("unknown");
  });
});

describe("parseChatMessage", () => {
  it("parses roles, tool calls, thinking and tool-result metadata", () => {
    const parsed = parseChatMessage(
      {
        role: "tool",
        content: "out",
        tool_call_id: "call-1",
        thinking: { thinking: "ponder" },
        tool_calls: [{ id: "t1", name: "read", arguments: { a: 1 } }, "junk", null],
        metadata: { extensions: { "chisel/tool_result_meta": { kind: "read" } } },
      },
      { src: "row" },
      7,
      Option.some(6),
      42,
    );
    expect(parsed.role).toBe("tool");
    expect(parsed.toolCalls).toHaveLength(1);
    expect(parsed.toolCalls[0]?.name).toBe("read");
    expect(Option.getOrNull(parsed.toolCallId)).toBe("call-1");
    expect(Option.getOrNull(parsed.toolName)).toBe("read");
    expect(Option.getOrNull(parsed.thinking)).toBe("ponder");
    expect(parsed.nodeId).toBe(7);
    expect(parsed.metadata).toEqual({ src: "row" });
  });

  it("coerces missing/odd fields to safe defaults", () => {
    const parsed = parseChatMessage({ content: { weird: true } }, null, 0, Option.none(), 0);
    expect(parsed.role).toBe("system");
    expect(parsed.content).toBe(JSON.stringify({ weird: true }));
    expect(parsed.toolCalls).toEqual([]);
    expect(Option.isNone(parsed.thinking)).toBe(true);
    expect(Option.isNone(parsed.toolName)).toBe(true);
    expect(Option.isNone(parsed.toolCallId)).toBe(true);
  });
});

describe("sessionFromDevinRow", () => {
  const baseRow = {
    id: "s1",
    workingDirectory: "/work",
    backendType: "windsurf",
    model: "claude",
    agentMode: "accept-edits",
    createdAt: 1,
    lastActivityAt: 2,
    title: null,
    mainChainId: null,
    shellLastSeenIndex: null,
    cogsJson: null,
    workspaceDirs: null,
    hidden: 0,
    metadata: null,
  };

  it("fills NULL columns with defaults and falls back to the id as title", () => {
    const session = sessionFromDevinRow(baseRow, [], []);
    expect(session.title).toBe("s1");
    expect(session.mainChainId).toBe(0);
    expect(session.shellLastSeenIndex).toBe(0);
    expect(session.cogsJson).toBe("[]");
    expect(session.metadata).toEqual({});
  });

  it("parses metadata json and survives malformed payloads", () => {
    expect(
      sessionFromDevinRow({ ...baseRow, metadata: '{"a":1}', title: "T" }, [], []).metadata,
    ).toEqual({ a: 1 });
    expect(sessionFromDevinRow({ ...baseRow, metadata: "not json" }, [], []).metadata).toEqual({});
  });
});

describe("default fixtures", () => {
  it("defaultSessionMetadata and defaultCogsJson produce valid JSON payloads", () => {
    const meta = defaultSessionMetadata();
    expect(meta.total_acu_cost).toBe(0);
    expect(meta.response_dimensions).toHaveLength(2);
    expect(JSON.parse(defaultCogsJson())).toHaveLength(4);
  });
});
