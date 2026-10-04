import { describe, expect, it } from "vite-plus/test";
import { parseSystemContext } from "../lib/systemContext";
import type { HistoryMessage } from "../lib/types";

const msg = (content: string, role: HistoryMessage["role"] = "system"): HistoryMessage => ({
  role,
  content,
  createdAt: 0,
});

const SYS_INFO = `<system_info>
Current workspace directories:
  /home/dev/api-server (cwd)
  /home/dev/lib
Platform: linux
OS Version: Ubuntu 24.04
Today's date: 2025-06-01
</system_info>`;

const RULES = `<rules type="always-on">
<rule name="global_rules" path="/home/dev/.rules/global.md">
always be nice
</rule>
<rule name="AGENTS" path="/repo/AGENTS.md">
project rules
</rule>
</rules>`;

describe("parseSystemContext", () => {
  it("returns an empty context for no messages", () => {
    expect(parseSystemContext([])).toEqual({
      workspaces: [],
      platform: null,
      osVersion: null,
      date: null,
      rules: [],
      reports: [],
      promptText: "",
    });
  });

  it("pulls subagent completion notifications out of promptText", () => {
    const ctx = parseSystemContext([
      msg("You are a helpful agent."),
      msg(
        "<subagent_completion_notification>\n[done]\n\n## Report\nAll good\n</subagent_completion_notification>",
      ),
    ]);
    expect(ctx.reports).toEqual(["[done]\n\n## Report\nAll good"]);
    expect(ctx.promptText).toBe("You are a helpful agent.");
  });

  it("dedupes repeated reports", () => {
    const notice = "<subagent_completion_notification>same</subagent_completion_notification>";
    const ctx = parseSystemContext([msg(notice), msg(notice)]);
    expect(ctx.reports).toEqual(["same"]);
  });

  it("extracts workspaces, fields, and rules out of system blocks", () => {
    const ctx = parseSystemContext([msg(`${SYS_INFO}\n${RULES}`)]);
    expect(ctx.workspaces).toEqual(["/home/dev/api-server", "/home/dev/lib"]);
    expect(ctx.platform).toBe("linux");
    expect(ctx.osVersion).toBe("Ubuntu 24.04");
    expect(ctx.date).toBe("2025-06-01");
    expect(ctx.rules).toEqual([
      { name: "global_rules", path: "/home/dev/.rules/global.md" },
      { name: "AGENTS", path: "/repo/AGENTS.md" },
    ]);
    expect(ctx.promptText).toBe("");
  });

  it("keeps the non-system remainder as promptText", () => {
    const ctx = parseSystemContext([
      msg(`${SYS_INFO}\nYou are a helpful agent.\n${RULES}\nFollow the rules.`),
      msg("Second paragraph."),
    ]);
    expect(ctx.promptText).toBe(
      "You are a helpful agent.\n\nFollow the rules.\n\nSecond paragraph.",
    );
  });

  it("dedupes workspaces and rules across repeated system nodes", () => {
    const ctx = parseSystemContext([msg(SYS_INFO), msg(`${SYS_INFO}\n${RULES}`), msg(RULES)]);
    expect(ctx.workspaces).toEqual(["/home/dev/api-server", "/home/dev/lib"]);
    expect(ctx.rules).toHaveLength(2);
  });

  it("stops the workspace block at the first unindented line", () => {
    const ctx = parseSystemContext([
      msg(`<system_info>
Current workspace directories:
  /ws/one
Platform: darwin
  /ws/not-a-workspace
</system_info>`),
    ]);
    expect(ctx.workspaces).toEqual(["/ws/one"]);
    expect(ctx.platform).toBe("darwin");
  });

  it("ignores fields outside the known set and non-matching lines", () => {
    const ctx = parseSystemContext([
      msg(`<system_info>
Hostname: box
Platform: mac
Shell: zsh
</system_info>`),
    ]);
    expect(ctx.platform).toBe("mac");
    expect(ctx.osVersion).toBeNull();
    expect(ctx.date).toBeNull();
  });

  it("treats later system_info blocks as a merge (last write wins per field)", () => {
    const ctx = parseSystemContext([
      msg("<system_info>\nPlatform: linux\n</system_info>"),
      msg("<system_info>\nPlatform: darwin\nToday's date: 2025-01-02\n</system_info>"),
    ]);
    expect(ctx.platform).toBe("darwin");
    expect(ctx.date).toBe("2025-01-02");
  });
});
