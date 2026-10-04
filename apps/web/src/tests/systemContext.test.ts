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

const SKILLS = `<available_skills>
The following skills can be invoked using the \`skill\` tool.

- **find-skills**: Helps users discover and install agent skills. (source: /home/dev/.claude/skills/find-skills/SKILL.md)
- **devin-cli**: Look up Devin CLI documentation (source: builtin:cli)
</available_skills>`;

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
    expect(ctx.reports).toEqual([{ title: "Report", body: "All good" }]);
    expect(ctx.promptText).toBe("You are a helpful agent.");
  });

  it("pulls agentId and at off the notification header and createdAt", () => {
    const notice =
      "<subagent_completion_notification>\n[Background subagent with agent_id=220067fd completed]\n\nAll mods migrated.\n\nDetails here.\n</subagent_completion_notification>";
    const ctx = parseSystemContext([msg(notice)]);
    expect(ctx.reports).toEqual([
      { title: "All mods migrated.", body: "Details here.", agentId: "220067fd" },
    ]);
    const timed = parseSystemContext([{ ...msg(notice), createdAt: 1750000000000 }]);
    expect(timed.reports[0]?.at).toBe(1750000000000);
  });

  it("keeps a one-line report as bare body (no title)", () => {
    const ctx = parseSystemContext([
      msg("<subagent_completion_notification>Done.</subagent_completion_notification>"),
    ]);
    expect(ctx.reports).toEqual([{ body: "Done." }]);
  });

  it("dedupes repeated reports", () => {
    const notice = "<subagent_completion_notification>same</subagent_completion_notification>";
    const ctx = parseSystemContext([msg(notice), msg(notice)]);
    expect(ctx.reports).toEqual([{ body: "same" }]);
  });

  it("extracts workspaces, fields, and rules out of system blocks", () => {
    const ctx = parseSystemContext([msg(`${SYS_INFO}\n${RULES}`)]);
    expect(ctx.workspaces).toEqual(["/home/dev/api-server", "/home/dev/lib"]);
    expect(ctx.platform).toBe("linux");
    expect(ctx.osVersion).toBe("Ubuntu 24.04");
    expect(ctx.date).toBe("2025-06-01");
    expect(ctx.rules).toEqual([
      { name: "global_rules", path: "/home/dev/.rules/global.md", content: "always be nice" },
      { name: "AGENTS", path: "/repo/AGENTS.md", content: "project rules" },
    ]);
    expect(ctx.promptText).toBe("");
  });

  it("omits rule content when the <rule> body is empty", () => {
    const ctx = parseSystemContext([
      msg(
        `<rules><rule name="empty" path="/r.md"></rule>\n<rule name="ws" path="/w.md">\nhi\n</rule></rules>`,
      ),
    ]);
    expect(ctx.rules).toEqual([
      { name: "empty", path: "/r.md" },
      { name: "ws", path: "/w.md", content: "hi" },
    ]);
  });

  it("extracts skills out of <available_skills> blocks", () => {
    const ctx = parseSystemContext([msg(`You are helpful.\n${SKILLS}`)]);
    expect(ctx.skills).toEqual([
      {
        name: "find-skills",
        description: "Helps users discover and install agent skills.",
        source: "/home/dev/.claude/skills/find-skills/SKILL.md",
      },
      { name: "devin-cli", description: "Look up Devin CLI documentation", source: "builtin:cli" },
    ]);
    expect(ctx.promptText).toBe("You are helpful.");
  });

  it("keeps skills without a (source: …) suffix and dedupes by name", () => {
    const block =
      "<available_skills>\n- **lint**: Run the linter\n- **lint**: dup\n</available_skills>";
    const ctx = parseSystemContext([msg(block), msg(block)]);
    expect(ctx.skills).toEqual([{ name: "lint", description: "Run the linter" }]);
  });

  it("splits promptText into sections at the minimum heading depth", () => {
    const prompt =
      "You are helpful.\n\n# Modes\nMode body.\n\n## Tone\nTone body.\n\n# Style\nStyle body.";
    const ctx = parseSystemContext([msg(prompt)]);
    expect(ctx.promptText).toBe(prompt);
    expect(ctx.promptSections).toEqual([
      { title: "Modes", body: "Mode body.\n\n## Tone\nTone body." },
      { title: "Style", body: "Style body." },
    ]);
  });

  it("ignores headings inside fenced code and reports no sections without headings", () => {
    const fenced = parseSystemContext([
      msg("Intro.\n\n```\n# not a heading\n```\n\n# Real\nBody."),
    ]);
    expect(fenced.promptSections).toEqual([{ title: "Real", body: "Body." }]);
    const plain = parseSystemContext([msg("No headings here.")]);
    expect(plain.promptSections).toBeUndefined();
    expect(plain.skills).toBeUndefined();
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
