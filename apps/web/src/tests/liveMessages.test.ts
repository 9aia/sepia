import { describe, expect, it } from "vite-plus/test";
import { applyAguiEvent } from "../lib/liveMessages";
import { toolSummary } from "../lib/toolDisplay";
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

  it("feeds the live args stream into toolSummary — the command lands on the row", () => {
    let m = applyAguiEvent(
      [],
      ev("TOOL_CALL_START", { toolCallId: "t1", toolCallName: "execute" }),
    );
    // Partial JSON deltas accumulate into `args` until the object closes.
    m = applyAguiEvent(m, ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: '{"command":"git' }));
    m = applyAguiEvent(m, ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: ' status"}' }));
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", status: "completed" }));

    const row = m[0];
    expect(row?.args).toBe('{"command":"git status"}');
    const display = toolSummary(row?.toolName ?? "tool", row?.args, row?.content ?? "");
    expect(display.label).toBe("Ran command");
    expect(display.detail).toBe("git status");
    expect(display.segments[0]).toEqual({ kind: "command", text: "git status" });
  });

  it("repeated rawInput snapshots concatenate — the last complete object wins", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_START", { toolCallId: "t1", toolCallName: "bash" }));
    // The Translator emits the full rawInput on tool_call and again on each
    // tool_call_update that carries it — snapshots, not deltas.
    m = applyAguiEvent(m, ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: '{"command":"ls"}' }));
    m = applyAguiEvent(
      m,
      ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: '{"command":"ls -la","timeout":40000}' }),
    );
    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1" }));

    const row = m[0];
    const display = toolSummary(row?.toolName ?? "tool", row?.args, row?.content ?? "");
    expect(display.detail).toBe("ls -la");
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

  it("folds contents — terminal refs and embedded blocks — into the live row", () => {
    let m = applyAguiEvent(
      [],
      ev("TOOL_CALL_START", {
        toolCallId: "t1",
        toolCallName: "exec",
        contents: [{ type: "terminal", terminalId: "term-1" }],
      }),
    );
    expect(m[0]?.contents).toEqual([{ type: "terminal", terminalId: "term-1" }]);

    // The mid-call custom event carries a fresh snapshot too.
    m = applyAguiEvent(
      m,
      ev("CUSTOM", {
        name: "acp:tool_call_update",
        value: { toolCallId: "t1", contents: [{ type: "text", text: "partial" }] },
      }),
    );
    expect(m[0]?.contents).toEqual([{ type: "text", text: "partial" }]);

    m = applyAguiEvent(
      m,
      ev("TOOL_CALL_END", {
        toolCallId: "t1",
        status: "completed",
        contents: [
          { type: "terminal", terminalId: "term-1", output: "done" },
          { type: "image", data: "aGk=", mimeType: "image/png" },
        ],
      }),
    );
    expect(m[0]?.contents).toEqual([
      { type: "terminal", terminalId: "term-1", output: "done" },
      { type: "image", data: "aGk=", mimeType: "image/png" },
    ]);
  });

  it("drops malformed content entries", () => {
    const m = applyAguiEvent(
      [],
      ev("TOOL_CALL_START", {
        toolCallId: "t1",
        contents: [
          { type: "terminal" },
          { type: "text" },
          { type: "image" },
          { type: "weird", x: 1 },
          "nope",
        ],
      }),
    );
    expect(m[0]?.contents).toBeUndefined();
  });

  it("synthesizes a tool row when updates arrive for an unknown id (attach mid-call)", () => {
    // The TOOL_CALL_START fired before we attached — args, the mid-call
    // custom snapshot and the end all land on a row that never started.
    let m = applyAguiEvent(
      [],
      ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: '{"command":"ls -la"}' }),
    );
    expect(m).toHaveLength(1);
    expect(m[0]?.role).toBe("tool");
    expect(m[0]?.args).toBe('{"command":"ls -la"}');
    expect(m[0]?.done).toBe(false);

    m = applyAguiEvent(
      m,
      ev("CUSTOM", {
        name: "acp:tool_call_update",
        value: { toolCallId: "t1", contents: [{ type: "terminal", terminalId: "term-1" }] },
      }),
    );
    expect(m).toHaveLength(1);
    expect(m[0]?.contents).toEqual([{ type: "terminal", terminalId: "term-1" }]);

    m = applyAguiEvent(m, ev("TOOL_CALL_END", { toolCallId: "t1", status: "completed" }));
    expect(m).toHaveLength(1);
    expect(m[0]?.done).toBe(true);
    expect(m[0]?.toolStatus).toBe("success");
  });

  it("names a synthesized row from the update's title — the id stands in otherwise", () => {
    // No name anywhere → the toolCallId renders as the placeholder name.
    let m = applyAguiEvent([], ev("TOOL_CALL_END", { toolCallId: "call-9" }));
    expect(m[0]?.toolName).toBe("call-9");
    expect(m[0]?.done).toBe(true);

    // A title on the args frame (forwarded from the ACP update) names it.
    m = applyAguiEvent(
      [],
      ev("TOOL_CALL_ARGS", {
        toolCallId: "t1",
        delta: "{}",
        toolCallName: "execute",
      }),
    );
    expect(m[0]?.toolName).toBe("execute");

    // …and on the mid-call custom event's `title`.
    m = applyAguiEvent(
      [],
      ev("CUSTOM", {
        name: "acp:tool_call_update",
        value: { toolCallId: "t2", title: "edit_file", diffs: [{ path: "/a", newText: "x" }] },
      }),
    );
    expect(m[0]?.toolName).toBe("edit_file");
    expect(m[0]?.diffs).toEqual([{ path: "/a", newText: "x" }]);
  });

  it("a late TOOL_CALL_START merges into a synthesized row instead of duplicating it", () => {
    let m = applyAguiEvent([], ev("TOOL_CALL_ARGS", { toolCallId: "t1", delta: '{"path":"/a"}' }));
    expect(m[0]?.toolName).toBe("t1");
    m = applyAguiEvent(
      m,
      ev("TOOL_CALL_START", {
        toolCallId: "t1",
        toolCallName: "read_file",
        locations: [{ path: "/a" }],
      }),
    );
    expect(m).toHaveLength(1);
    expect(m[0]?.toolName).toBe("read_file");
    expect(m[0]?.args).toBe('{"path":"/a"}');
    expect(m[0]?.locations).toEqual([{ path: "/a" }]);
  });

  it("a duplicate START leaves a name it doesn't carry alone", () => {
    let m = applyAguiEvent(
      [],
      ev("TOOL_CALL_END", { toolCallId: "t1", toolCallName: "bash", status: "completed" }),
    );
    m = applyAguiEvent(m, ev("TOOL_CALL_START", { toolCallId: "t1" }));
    expect(m).toHaveLength(1);
    expect(m[0]?.toolName).toBe("bash");
    expect(m[0]?.done).toBe(true);
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
