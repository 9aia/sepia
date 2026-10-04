import { describe, expect, it } from "vite-plus/test";
import { applyAguiEvent } from "../lib/liveMessages";
import type { AgUiEvent } from "../lib/api";

const ev = (type: string, extra: Record<string, unknown> = {}): AgUiEvent =>
  ({ type, ...extra }) as AgUiEvent;

describe("applyAguiEvent", () => {
  it("builds an assistant message from start/content/end", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m1" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_CONTENT", { messageId: "m1", delta: "hel" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_CONTENT", { messageId: "m1", delta: "lo" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_END", { messageId: "m1" }));
    expect(m).toHaveLength(1);
    expect(m[0]?.content).toBe("hello");
    expect(m[0]?.done).toBe(true);
  });

  it("tracks independent message ids", () => {
    let m = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "a" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_START", { messageId: "b" }));
    m = applyAguiEvent(m, ev("TEXT_MESSAGE_CONTENT", { messageId: "b", delta: "x" }));
    expect(m[0]?.content).toBe("");
    expect(m[1]?.content).toBe("x");
  });

  it("synthesizes an id when messageId is absent", () => {
    const m = applyAguiEvent([], ev("TEXT_MESSAGE_START", {}));
    expect(m[0]?.id).toBe("text-0");
  });

  it("updates tool calls through args → result → end", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1", toolCallName: "read" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: "{}" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_RESULT", { toolCallId: "t1", content: "ok" }));
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1" }));
    expect(m[0]?.role).toBe("tool");
    expect(m[0]?.toolName).toBe("read");
    // Args stay in their own field so each side renders on its own.
    expect(m[0]?.args).toBe("{}");
    expect(m[0]?.content).toBe("ok");
    expect(m[0]?.done).toBe(true);
  });

  it("folds locations/diffs from tool events into the live row", () => {
    let m = applyAguiEvent(
      [],
      ev("TOOL_CALL_START", {
        toolCallId: "t1",
        toolCallName: "edit_file",
        locations: [{ path: "/a", line: 3 }],
        diffs: [{ path: "/a", oldText: "x", newText: "y" }],
      }),
    );
    expect(m[0]?.locations).toEqual([{ path: "/a", line: 3 }]);
    expect(m[0]?.diffs).toEqual([{ path: "/a", oldText: "x", newText: "y" }]);

    // A mid-call snapshot rides the custom event; the latest payload wins.
    m = applyAguiEvent(
      m,
      ev("CUSTOM", {
        name: "acp:tool_call_update",
        value: { toolCallId: "t1", diffs: [{ path: "/b", newText: "created" }] },
      }),
    );
    expect(m[0]?.diffs).toEqual([{ path: "/b", newText: "created" }]);
    expect(m[0]?.locations).toEqual([{ path: "/a", line: 3 }]);

    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", status: "completed" }));
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.toolStatus).toBe("success");
    expect(m[0]?.diffs).toEqual([{ path: "/b", newText: "created" }]);
  });

  it("content for a missing messageId is a no-op", () => {
    const m = applyAguiEvent([], ev("TEXT_MESSAGE_CONTENT", { messageId: "ghost", delta: "x" }));
    expect(m).toHaveLength(0);
  });

  it("run end produces a status row so the turn boundary shows", () => {
    const done = applyAguiEvent([], ev("RUN_FINISHED"));
    expect(done[0]?.role).toBe("status");
    expect(done[0]?.content).toBe("Run finished");
    const failed = applyAguiEvent([], ev("RUN_ERROR"));
    expect(failed[0]?.content).toBe("Run failed");
  });

  it("ignores lifecycle/unknown events without mutating", () => {
    const before = applyAguiEvent([], ev("TEXT_MESSAGE_START", { messageId: "m" }));
    expect(applyAguiEvent(before, ev("RUN_STARTED"))).toHaveLength(1);
    expect(applyAguiEvent(before, ev("RUN_CANCELLED"))).toHaveLength(1);
    expect(applyAguiEvent(before, ev("CUSTOM", { name: "x" }))).toHaveLength(1);
  });
});
