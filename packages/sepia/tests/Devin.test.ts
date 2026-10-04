import { describe, expect, it } from "vite-plus/test";
import { Option } from "effect";
import {
  applyToolCallOutcomes,
  buildChatMessage,
  defaultCogsJson,
  defaultSessionMetadata,
  fromAcpToolCallStatus,
  parseChatMessage,
  sessionFromDevinRow,
  toAcpToolCallStatus,
  toolNodeOutcomes,
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

  it("persists usage, request, model and finish reason the node carries", () => {
    const msg = buildChatMessage(
      node({
        role: "assistant",
        usage: Option.some({ input: 100, output: 10, cacheRead: 50, cacheWrite: 5 }),
        requestId: Option.some("req-1"),
        finishReason: Option.some("length"),
        model: Option.some("swe-1-7-high"),
      }),
      "fallback-model",
    ) as Record<string, unknown>;
    const meta = msg.metadata as Record<string, unknown>;
    expect(meta.num_tokens).toBe(10);
    expect(meta.request_id).toBe("req-1");
    expect(meta.finish_reason).toBe("length");
    expect(meta.generation_model).toBe("swe-1-7-high");
    expect(meta.metrics).toEqual({
      input_tokens: 100,
      output_tokens: 10,
      cache_read_tokens: 50,
      cache_creation_tokens: 5,
    });

    // usage without cache counters writes explicit nulls
    const sparse = buildChatMessage(
      node({ role: "assistant", usage: Option.some({ input: 3, output: 1 }) }),
      "m",
    ) as { metadata: { metrics: Record<string, unknown> } };
    expect(sparse.metadata.metrics).toEqual({
      input_tokens: 3,
      output_tokens: 1,
      cache_read_tokens: null,
      cache_creation_tokens: null,
    });
  });

  it("maps a stored tool-call status back to ACP and emits result extensions", () => {
    const rendered = node({
      role: "assistant",
      toolCalls: [
        ToolCall.make({
          id: "t1",
          name: "exec",
          arguments: {},
          status: Option.some("error" as const),
        }),
      ],
      metadata: { summarized_from: 1 },
    });
    const assistant = buildChatMessage(rendered, "m") as {
      metadata: { extensions: { "chisel/tool_call_content": Record<string, { status: string }> } };
    };
    expect(assistant.metadata.extensions["chisel/tool_call_content"].t1.status).toBe("failed");

    const tool = buildChatMessage(
      node({
        role: "tool",
        toolName: Option.some("exec"),
        toolCallId: Option.some("t1"),
        toolResult: Option.some({ status: "error", exitCode: 2, durationMs: 42 }),
      }),
      "m",
    ) as { metadata: { extensions: Record<string, any>; metrics: unknown; request_id: unknown } };
    const ext = tool.metadata.extensions;
    expect(ext["chisel/tool_result_meta"]).toEqual({ success: false, kind: "exec" });
    expect(ext["chisel/terminal_output"]).toEqual({ exit: { exit_code: 2 } });
    expect(ext["chisel/tool_call_timing"]).toEqual({ duration_ms: 42 });
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
    expect(Option.isNone(parsed.usage)).toBe(true);
    expect(Option.isNone(parsed.model)).toBe(true);
    expect(Option.isNone(parsed.requestId)).toBe(true);
    expect(Option.isNone(parsed.finishReason)).toBe(true);
    expect(Option.isNone(parsed.toolResult)).toBe(true);
  });

  it("reads token metrics, request id, model and finish reason", () => {
    const parsed = parseChatMessage(
      {
        role: "assistant",
        content: "hi",
        metadata: {
          num_tokens: 10,
          request_id: "req-1",
          finish_reason: "tool_calls",
          generation_model: "swe-1-7-medium",
          metrics: {
            ttft_ms: 843,
            input_tokens: 4727,
            output_tokens: 155,
            cache_read_tokens: 13312,
            cache_creation_tokens: null,
          },
        },
      },
      null,
      0,
      Option.none(),
      0,
    );
    expect(Option.getOrNull(parsed.usage)).toEqual({
      input: 4727,
      output: 155,
      cacheRead: 13312,
    });
    expect(Option.getOrNull(parsed.model)).toBe("swe-1-7-medium");
    expect(Option.getOrNull(parsed.requestId)).toBe("req-1");
    expect(Option.getOrNull(parsed.finishReason)).toBe("tool_calls");
  });

  it("leaves usage empty for non-object or token-less metrics", () => {
    const parse = (metrics: unknown) =>
      parseChatMessage(
        { role: "assistant", content: "", metadata: { metrics } },
        null,
        0,
        Option.none(),
        0,
      ).usage;
    expect(Option.isNone(parse("nope"))).toBe(true);
    expect(Option.isNone(parse({}))).toBe(true);
    expect(Option.isNone(parse({ ttft_ms: 10 }))).toBe(true);
    // output only still counts
    expect(Option.getOrNull(parse({ output_tokens: 7 }))).toEqual({ input: 0, output: 7 });
    expect(Option.getOrNull(parse({ input_tokens: 9, cache_creation_tokens: 3 }))).toEqual({
      input: 9,
      output: 0,
      cacheWrite: 3,
    });
  });

  it("maps chisel tool-call statuses onto the calls", () => {
    const parsed = parseChatMessage(
      {
        role: "assistant",
        content: "",
        tool_calls: [
          { id: "done", name: "exec", arguments: {} },
          { id: "running", name: "exec", arguments: {} },
          { id: "busted", name: "exec", arguments: {} },
          { id: "weird", name: "exec", arguments: {} },
          {},
        ],
        metadata: {
          extensions: {
            "chisel/tool_call_content": {
              done: { status: "completed" },
              running: { status: "in_progress" },
              busted: { status: "failed" },
              weird: { status: "surprising" },
              nullish: null,
            },
          },
        },
      },
      null,
      0,
      Option.none(),
      0,
    );
    expect(parsed.toolCalls.map((tc) => Option.getOrNull(tc.status))).toEqual([
      "success",
      "pending",
      "error",
      null,
      null,
    ]);
    // a call entry with no fields still parses to safe defaults
    const bare = parsed.toolCalls[4];
    expect(bare?.id).toBe("");
    expect(bare?.name).toBe("unknown");
    expect(bare?.arguments).toEqual({});
  });

  it("serializes a missing content field as an empty string", () => {
    const parsed = parseChatMessage({ role: "user" }, null, 0, Option.none(), 0);
    expect(parsed.content).toBe('""');
  });

  it("keeps the recorded outcome of a tool result node", () => {
    const parsed = parseChatMessage(
      {
        role: "tool",
        content: "boom",
        tool_call_id: "c1",
        metadata: {
          extensions: {
            "chisel/tool_result_meta": { success: false, kind: "exec" },
            "chisel/terminal_output": { exit: { terminal_id: "t0", exit_code: 2 } },
            "chisel/tool_call_timing": { duration_ms: 42 },
          },
        },
      },
      null,
      0,
      Option.none(),
      0,
    );
    expect(Option.getOrNull(parsed.toolResult)).toEqual({
      status: "error",
      exitCode: 2,
      durationMs: 42,
    });
    expect(Option.getOrNull(parsed.toolName)).toBe("exec");
  });

  it("treats a recorded success without extras as a clean result", () => {
    const parsed = parseChatMessage(
      {
        role: "tool",
        content: "ok",
        tool_call_id: "c1",
        metadata: { extensions: { "chisel/tool_result_meta": { success: true } } },
      },
      null,
      0,
      Option.none(),
      0,
    );
    expect(Option.getOrNull(parsed.toolResult)).toEqual({ status: "success" });
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

  it("carries sub-agent lineage when the store records it", () => {
    const session = sessionFromDevinRow(baseRow, [], [], {
      parentSessionId: "parent-1",
      agentId: "agent-x",
    });
    expect(Option.getOrNull(session.parentSessionId)).toBe("parent-1");
    expect(Option.getOrNull(session.agentId)).toBe("agent-x");

    const plain = sessionFromDevinRow(baseRow, [], []);
    expect(Option.isNone(plain.parentSessionId)).toBe(true);
    expect(Option.isNone(plain.agentId)).toBe(true);
  });
});

describe("tool call status mapping", () => {
  it("maps ACP statuses to the IR lifecycle and back", () => {
    expect(fromAcpToolCallStatus("completed")).toBe("success");
    expect(fromAcpToolCallStatus("failed")).toBe("error");
    expect(fromAcpToolCallStatus("pending")).toBe("pending");
    expect(fromAcpToolCallStatus("in_progress")).toBe("pending");
    expect(fromAcpToolCallStatus("bogus")).toBeUndefined();
    expect(fromAcpToolCallStatus(null)).toBeUndefined();

    expect(toAcpToolCallStatus("success")).toBe("completed");
    expect(toAcpToolCallStatus("error")).toBe("failed");
    expect(toAcpToolCallStatus("pending")).toBe("pending");
  });
});

describe("tool call outcomes", () => {
  const assistantWith = (...calls: ReadonlyArray<ToolCall>) =>
    node({ role: "assistant", toolCalls: calls });

  it("collects outcomes from tool nodes and folds them onto calls", () => {
    const nodes = [
      assistantWith(
        ToolCall.make({ id: "c1", name: "exec", arguments: {} }),
        ToolCall.make({ id: "c2", name: "read", arguments: {} }),
      ),
      node({ role: "user", content: "unrelated" }),
      node({
        role: "tool",
        toolCallId: Option.some("c1"),
        toolResult: Option.some({ status: "error", exitCode: 2, durationMs: 9 }),
      }),
      // a result with no outcome info contributes nothing
      node({ role: "tool", toolCallId: Option.some("c2") }),
      // calls no outcome names keep their node untouched
      assistantWith(ToolCall.make({ id: "c9", name: "exec", arguments: {} })),
    ];

    const outcomes = toolNodeOutcomes(nodes);
    expect(outcomes.get("c1")).toEqual({ status: "error", exitCode: 2, durationMs: 9 });
    expect(outcomes.has("c2")).toBe(false);

    const enriched = applyToolCallOutcomes(nodes, outcomes);
    const calls = enriched[0]!.toolCalls;
    expect(Option.getOrNull(calls[0]!.status)).toBe("error");
    expect(Option.getOrNull(calls[0]!.exitCode)).toBe(2);
    expect(Option.getOrNull(calls[0]!.durationMs)).toBe(9);
    expect(Option.isNone(calls[1]!.status)).toBe(true);
    // untouched nodes keep their identity
    expect(enriched[1]).toBe(nodes[1]);
    expect(enriched[2]).toBe(nodes[2]);
    expect(enriched[4]).toBe(nodes[4]);
  });

  it("returns the input unchanged when nothing was recorded", () => {
    const nodes = [assistantWith(ToolCall.make({ id: "c1", name: "exec", arguments: {} }))];
    expect(applyToolCallOutcomes(nodes, new Map())).toBe(nodes);
    // a node with no calls is returned as-is even with outcomes around
    const plain = [node({ role: "user" })];
    const outcomes = new Map([["c1", { status: "success" as const }]]);
    expect(applyToolCallOutcomes(plain, outcomes)[0]).toBe(plain[0]);
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
