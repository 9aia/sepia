import { describe, expect, it, vi } from "vite-plus/test";
import { StderrTail } from "../src/stderr.js";

describe("StderrTail", () => {
  it("buffers partial lines across pushes", () => {
    const tail = new StderrTail({ id: "devin" });
    tail.push("partial");
    expect(tail.recent()).toEqual([]);
    tail.push(" line\nnext\n");
    expect(tail.recent()).toEqual(["[agent:devin] partial line", "[agent:devin] next"]);
  });

  it("truncates lines past 500 chars", () => {
    const tail = new StderrTail({ id: "c" });
    tail.push(`${"x".repeat(600)}\n`);
    const [line] = tail.recent();
    expect(line).toBe(`[agent:c] ${"x".repeat(500)}`);
  });

  it("keeps only the last 100 lines in the ring", () => {
    const tail = new StderrTail({ id: "c" });
    for (let i = 0; i < 110; i++) tail.push(`line ${i}\n`);
    const recent = tail.recent();
    expect(recent).toHaveLength(100);
    expect(recent[0]).toBe("[agent:c] line 10");
    expect(recent[99]).toBe("[agent:c] line 109");
  });

  it("forwards to the sink only in debug mode", () => {
    const sink = vi.fn();
    const quiet = new StderrTail({ id: "c", sink });
    quiet.push("hidden\n");
    expect(sink).not.toHaveBeenCalled();

    const loud = new StderrTail({ id: "c", debug: true, sink });
    loud.push("shown\n");
    expect(sink).toHaveBeenCalledWith("[agent:c] shown");
  });

  it("defaults the sink to process.stderr", () => {
    const write = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    try {
      const tail = new StderrTail({ id: "c", debug: true });
      tail.push("via stderr\n");
      expect(write).toHaveBeenCalledWith("[agent:c] via stderr\n");
    } finally {
      write.mockRestore();
    }
  });
});
