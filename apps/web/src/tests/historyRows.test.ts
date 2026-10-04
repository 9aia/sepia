import { describe, expect, it } from "vite-plus/test";
import { buildRows, liveCoveredByHistory } from "../lib/historyRows";
import type { HistoryMessage } from "../lib/types";
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
