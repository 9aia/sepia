/** Branch arms: option defaults, location/diff omission, stderr sink. */
import { afterEach, expect, test, vi } from "vite-plus/test";
import { normalizePermission, normalizeUpdate } from "../src/normalize.js";
import { StderrTail } from "../src/stderr.js";
import { resolveAgent } from "../src/AgentRegistry.js";

afterEach(() => {
  vi.restoreAllMocks();
});

test("permission options with missing fields default to empty strings", () => {
  const perm = normalizePermission({
    toolCallId: "t1",
    options: [{ optionId: "o1" }, { name: "deny", kind: "reject" }, {}],
  });
  expect(perm?.options).toEqual([
    { optionId: "o1", name: "", kind: "" },
    { optionId: "", name: "deny", kind: "reject" },
    { optionId: "", name: "", kind: "" },
  ]);
});

test("tool_call locations omit line and diffs omit absent texts", () => {
  const update = normalizeUpdate({
    sessionUpdate: "tool_call",
    toolCallId: "t1",
    title: "edit",
    locations: [{ path: "/a" }, { path: "/b", line: 9 }, { line: 2 }],
    content: [
      { type: "diff", path: "/a" },
      { type: "diff", path: "/b", oldText: "o" },
      { type: "diff", newText: "n" }, // no path → dropped
      { type: "other", x: 1 },
    ],
  });
  expect(update.kind).toBe("tool_call");
  if (update.kind !== "tool_call") return;
  expect(update.locations).toEqual([
    { path: "/a" },
    { path: "/b", line: 9 },
    { path: "", line: 2 },
  ]);
  expect(update.diffs).toEqual([{ path: "/a" }, { path: "/b", oldText: "o" }]);
});

test("tool_call content entries: terminal by id, text blocks, drops", () => {
  const update = normalizeUpdate({
    sessionUpdate: "tool_call",
    toolCallId: "t1",
    title: "run",
    content: [
      { type: "terminal", id: "term-9", output: "out" },
      { type: "terminal" }, // no id → dropped
      { type: "content", content: { type: "text", text: "t" } },
      { type: "content", content: { type: "image", uri: "file:///i" } },
      { type: "content", content: { type: "audio", data: "x" } }, // unsupported → dropped
      { type: "content", content: { type: "text" } }, // no text → dropped
      42, // non-record → dropped
    ],
  });
  if (update.kind !== "tool_call") throw new Error("bad kind");
  expect(update.contents).toEqual([
    { type: "terminal", terminalId: "term-9", output: "out" },
    { type: "text", text: "t" },
    { type: "image", uri: "file:///i" },
  ]);
});

test("resolveAgent throws with the known list", () => {
  expect(() => resolveAgent("bogus")).toThrow(/Unknown agent "bogus"; known agents:/);
});

test("StderrTail writes to process.stderr when debug is on and no sink given", () => {
  const spy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  const tail = new StderrTail({ id: "x", debug: true });
  tail.push("a line\n");
  expect(spy).toHaveBeenCalledWith("[agent:x] a line\n");
  // debug off → sink never called
  spy.mockClear();
  const quiet = new StderrTail({ id: "x" });
  quiet.push("hidden\n");
  expect(quiet.recent()).toEqual(["[agent:x] hidden"]);
  expect(spy).not.toHaveBeenCalled();
});
