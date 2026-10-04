import { describe, expect, it } from "vite-plus/test";
import { buildRows, liveCoveredByHistory } from "../lib/historyRows";
import type { HistoryMessage, RunSpan } from "../lib/types";
import type { LiveMessage } from "../lib/liveMessages";
import type { SystemContext } from "../lib/systemContext";

const msg = (role: HistoryMessage["role"], content: string, createdAt = 0): HistoryMessage => ({
  role,
  content,
  createdAt,
  toolName: undefined,
});

const live = (role: LiveMessage["role"], content: string, id: string = role): LiveMessage => ({
  id,
  role,
  content,
  done: true,
});

const emptyContext: SystemContext = {
  workspaces: [],
  platform: null,
  osVersion: null,
  date: null,
  rules: [],
  reports: [],
  promptText: "",
};

const ctx = (over: Partial<SystemContext>): SystemContext => ({ ...emptyContext, ...over });

describe("buildRows", () => {
  it("returns history rows for a plain conversation", () => {
    const rows = buildRows([msg("user", "hi"), msg("assistant", "hello")], [], emptyContext);
    expect(rows).toHaveLength(2);
    expect(rows.map((r) => r.kind)).toEqual(["history", "history"]);
  });

  it("skips system messages from conversation rows", () => {
    const rows = buildRows(
      [msg("system", "<system_info>x"), msg("user", "hi"), msg("system", "rules")],
      [],
      ctx({ promptText: "p" }),
    );
    expect(rows[0]?.kind).toBe("system");
    expect(rows.filter((r) => r.kind === "history")).toHaveLength(1);
  });

  it("collapses back-to-back duplicate messages (devin context rewrites)", () => {
    const rows = buildRows(
      [
        msg("system", "ctx"),
        msg("user", "hello"),
        msg("system", "ctx"),
        msg("user", "hello"),
        msg("system", "ctx"),
        msg("user", "hello"),
        msg("assistant", "reply"),
        msg("assistant", "reply"),
      ],
      [],
      emptyContext,
    );
    const texts = rows.filter((r) => r.kind === "history");
    expect(texts).toHaveLength(2);
  });

  it("keeps a repeat when a different message intervenes", () => {
    const rows = buildRows(
      [msg("user", "ok"), msg("assistant", "done"), msg("user", "ok")],
      [],
      emptyContext,
    );
    expect(rows.filter((r) => r.kind === "history")).toHaveLength(3);
  });

  it("keeps same content across different roles", () => {
    const rows = buildRows([msg("user", "x"), msg("assistant", "x")], [], emptyContext);
    expect(rows.filter((r) => r.kind === "history")).toHaveLength(2);
  });

  it("drops empty assistant nodes (tool-call carriers)", () => {
    const tool: HistoryMessage = { role: "tool", content: "out", createdAt: 1, toolName: "exec" };
    const rows = buildRows(
      [msg("assistant", ""), tool, msg("assistant", "  "), msg("assistant", "answer")],
      [],
      emptyContext,
    );
    const texts = rows.filter((r) => r.kind === "history");
    expect(texts).toHaveLength(2);
    expect(texts[0]?.kind === "history" && texts[0].message.role).toBe("tool");
  });

  it("keeps empty user and tool messages", () => {
    const rows = buildRows([msg("user", ""), msg("tool", "")], [], emptyContext);
    expect(rows.filter((r) => r.kind === "history")).toHaveLength(2);
  });

  it("emits no context row when the parse is empty", () => {
    const rows = buildRows([msg("system", ""), msg("user", "hi")], [], emptyContext);
    expect(rows.map((r) => r.kind)).toEqual(["history"]);
  });

  it("appends live rows after history", () => {
    const rows = buildRows(
      [msg("user", "old")],
      [live("user", "new"), live("assistant", "stream", "a")],
      emptyContext,
    );
    expect(rows.map((r) => r.kind)).toEqual(["history", "live", "live"]);
  });

  it("returns empty for a fresh session", () => {
    expect(buildRows([], [], emptyContext)).toHaveLength(0);
  });
});

const span = (agent: string, node: string, at: number): RunSpan => ({ at, agent, node });
const spanLabel = (s: RunSpan): string => `${s.agent}@${s.node}`;

describe("buildRows — run span markers", () => {
  it("labels the first segment at the top and each later span at its boundary", () => {
    const rows = buildRows(
      [msg("user", "early", 10), msg("assistant", "mid", 20), msg("user", "late", 30)],
      [],
      emptyContext,
      [span("devin", "node-a", 5), span("cline", "node-b", 25)],
      spanLabel,
    );
    expect(rows.map((r) => r.kind)).toEqual(["span", "history", "history", "span", "history"]);
    expect(rows[0]).toMatchObject({ label: "devin@node-a", at: 5 });
    expect(rows[3]).toMatchObject({ label: "cline@node-b", at: 25 });
    // The boundary lands before the first message at/after the span's `at`.
    expect(rows[4]).toMatchObject({ kind: "history" });
    expect(rows[4]?.kind === "history" && rows[4].message.content).toBe("late");
  });

  it("a message exactly at the span timestamp belongs to the new segment", () => {
    const rows = buildRows(
      [msg("user", "before", 10), msg("user", "boundary", 20)],
      [],
      emptyContext,
      [span("devin", "n", 5), span("cline", "n", 20)],
      spanLabel,
    );
    expect(rows.map((r) => r.kind)).toEqual(["span", "history", "span", "history"]);
  });

  it("draws no markers until a session has at least two spans", () => {
    const rows = buildRows(
      [msg("user", "hi", 10)],
      [],
      emptyContext,
      [span("devin", "node-a", 5)],
      spanLabel,
    );
    expect(rows.map((r) => r.kind)).toEqual(["history"]);
  });

  it("a span newer than the backlog trails history and labels live rows", () => {
    const rows = buildRows(
      [msg("user", "old", 10)],
      [live("user", "streaming", "l1")],
      emptyContext,
      [span("devin", "node-a", 1), span("cline", "node-b", 999)],
      spanLabel,
    );
    expect(rows.map((r) => r.kind)).toEqual(["span", "history", "span", "live"]);
    expect(rows[2]).toMatchObject({ label: "cline@node-b" });
  });

  it("orders unsorted spans by their timestamp", () => {
    const rows = buildRows(
      [msg("user", "a", 10), msg("user", "b", 30)],
      [],
      emptyContext,
      [span("cline", "node-b", 20), span("devin", "node-a", 5)],
      spanLabel,
    );
    expect(rows.map((r) => (r.kind === "span" ? r.label : r.kind))).toEqual([
      "devin@node-a",
      "history",
      "cline@node-b",
      "history",
    ]);
  });

  it("stacked spans with no messages between them each get a marker", () => {
    const rows = buildRows(
      [msg("user", "only", 30)],
      [],
      emptyContext,
      [span("devin", "a", 5), span("cline", "b", 10), span("devin", "a", 15)],
      spanLabel,
    );
    expect(rows.map((r) => (r.kind === "span" ? r.label : r.kind))).toEqual([
      "devin@a",
      "cline@b",
      "devin@a",
      "history",
    ]);
  });
});

describe("liveCoveredByHistory", () => {
  it("true when empty", () => {
    expect(liveCoveredByHistory([], [])).toBe(true);
  });

  it("true when every user/assistant live text is in history", () => {
    const l = [live("user", "hi"), live("assistant", "yo")];
    expect(liveCoveredByHistory(l, [msg("user", "hi"), msg("assistant", "yo")])).toBe(true);
  });

  it("false when a live user text is not stored yet (flush lag)", () => {
    const l = [live("user", "just sent")];
    expect(liveCoveredByHistory(l, [msg("user", "older")])).toBe(false);
  });

  it("ignores tool and reasoning rows — history supersedes them", () => {
    const l = [live("tool", "result"), live("reasoning", "thinking")];
    expect(liveCoveredByHistory(l, [])).toBe(true);
  });

  it("blocks clearing when only one of several is covered", () => {
    const l = [live("user", "hi"), live("assistant", "not yet")];
    expect(liveCoveredByHistory(l, [msg("user", "hi")])).toBe(false);
  });

  it("does not match a user text stored under a different role", () => {
    const l = [live("user", "hi")];
    expect(liveCoveredByHistory(l, [msg("assistant", "hi")])).toBe(false);
  });
});
