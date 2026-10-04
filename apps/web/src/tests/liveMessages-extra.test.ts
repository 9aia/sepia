import { describe, expect, it } from "vite-plus/test";
import { applyAguiEvent } from "../lib/liveMessages";
import type { AgUiEvent } from "../lib/api";

const ev = (type: string, extra: Record<string, unknown> = {}): AgUiEvent =>
  ({ type, ...extra }) as AgUiEvent;

describe("applyAguiEvent — edge paths", () => {
  it("builds a reasoning message from start/content/end", () => {
    let m = applyAguiEvent([], ev("REASONING_MESSAGE_START", { messageId: "r1" }));
    m = applyAguiEvent(m, ev("REASONING_MESSAGE_CONTENT", { messageId: "r1", delta: "think" }));
    m = applyAguiEvent(m, ev("REASONING_MESSAGE_END", { messageId: "r1" }));
    expect(m[0]?.role).toBe("reasoning");
    expect(m[0]?.content).toBe("think");
    expect(m[0]?.done).toBe(true);
  });

  it("synthesizes a reasoning id when messageId is absent", () => {
    const m = applyAguiEvent([], ev("REASONING_MESSAGE_START", {}));
    expect(m[0]?.id).toBe("reasoning-0");
  });

  it("stringifies non-string tool results", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_RESULT", { toolCallId: "t1", content: { exit: 0 } }));
    expect(m[0]?.content).toBe('{"exit":0}');
    m = applyAguiEvent(m, ev("TOOL_CALL_RESULT", { toolCallId: "t1" }));
    expect(m[0]?.content).toBe('{"exit":0}""');
  });

  it("synthesizes a tool id when toolCallId is absent", () => {
    const m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallName: "bash" }));
    expect(m[0]?.id).toBe("tool-0");
    expect(m[0]?.toolName).toBe("bash");
  });

  it("a duplicate TEXT_MESSAGE_START id is ignored", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_CONTENT", { messageId: "m1", delta: "x" }));
    const again = applyAguiEvent(m, ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    expect(again).toHaveLength(1);
    expect(again[0]?.content).toBe("x");
  });

  it("updates for unknown or missing ids are no-ops", () => {
    const base = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    expect(applyAguiEvent(base, ev("TEXT_MESSAGE_END", { messageId: "ghost" }))).toHaveLength(1);
    expect(applyAguiEvent(base, ev("TEXT_MESSAGE_CONTENT", { delta: "x" }))).toHaveLength(1);
    expect(
      applyAguiEvent(base, ev("TOOL_CALL_ARGS", { toolCallId: "ghost", delta: "{}" })),
    ).toHaveLength(1);
    expect(applyAguiEvent(base, ev("TOOL_CALL_END", { toolCallId: "ghost" }))).toHaveLength(1);
    expect(
      applyAguiEvent(base, ev("REASONING_MESSAGE_CONTENT", { messageId: "ghost" })),
    ).toHaveLength(1);
    expect(base[0]?.done).toBe(false);
    expect(base[0]?.content).toBe("");
  });

  it("non-string deltas and ids don't break the fold", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: 42 }));
    // messageId 42 is not a string — the fallback id kicks in.
    expect(m[0]?.id).toBe("text-0");
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_CONTENT", { messageId: "text-0", delta: 7 }));
    expect(m[0]?.content).toBe("");
  });

  it("TOOL_CALL_END picks up IR v2 fields when they ride along", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1" }));
    m = applyAguiEvent(
      m,
      ev("TOOL_CALL_END", { toolCallId: "t1", toolStatus: "error", exitCode: 2, durationMs: 1200 }),
    );
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.toolStatus).toBe("error");
    expect(m[0]?.exitCode).toBe(2);
    expect(m[0]?.durationMs).toBe(1200);
  });

  it("TOOL_CALL_END maps ACP-style statuses onto the IR names", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", status: "failed" }));
    expect(m[0]?.toolStatus).toBe("error");
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", status: "completed" }));
    expect(m[0]?.toolStatus).toBe("success");
  });

  it("TOOL_CALL_END without v2 fields just closes the call", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", exitCode: "nope" }));
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.toolStatus).toBeUndefined();
    expect(m[0]?.exitCode).toBeUndefined();
  });

  it("TEXT_MESSAGE_END picks up usage, model, and finishReason", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    m = applyAguiEvent(
      m,
      ev("TEXT_MESSAGE_END", {
        messageId: "m1",
        usage: { input: 1200, output: 340, cacheRead: 800, cost: 0.02, bogus: "x" },
        model: "claude-sonnet",
        finishReason: "length",
      }),
    );
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.usage).toEqual({ input: 1200, output: 340, cacheRead: 800, cost: 0.02 });
    expect(m[0]?.model).toBe("claude-sonnet");
    expect(m[0]?.finishReason).toBe("length");
  });

  it("TEXT_MESSAGE_END ignores malformed usage", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_END", { messageId: "m1", usage: { input: "x" } }));
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.usage).toBeUndefined();
  });

  it("unhandled events return an equivalent copy", () => {
    const base = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    const next = applyAguiEvent(base, ev("SOME_FUTURE_EVENT"));
    expect(next).not.toBe(base);
    expect(next).toEqual(base);
  });
});
