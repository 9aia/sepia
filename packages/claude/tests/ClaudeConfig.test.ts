import { Effect, Option } from "effect";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vite-plus/test";
import {
  AgentConfig,
  ConfigAgent,
  ConfigCommand,
  ConfigHook,
  ConfigRule,
  ConfigSkill,
} from "sepia-core";
import * as ClaudeConfig from "../src/ClaudeConfig.js";

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-claude-config-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

/** A project-shaped fixture: `<root>/.claude` plus project-root files. */
const fixture = (root: string): string => {
  const dir = join(root, ".claude");
  mkdirSync(join(dir, "skills", "pnpm", "references"), { recursive: true });
  mkdirSync(join(dir, "commands"), { recursive: true });
  mkdirSync(join(dir, "agents"), { recursive: true });
  writeFileSync(
    join(dir, "skills", "pnpm", "SKILL.md"),
    "---\nname: pnpm\ndescription: use pnpm\nsubagent: true\n---\nUse pnpm.\n",
  );
  writeFileSync(join(dir, "skills", "pnpm", "references", "core.md"), "ref");
  writeFileSync(
    join(dir, "commands", "review.md"),
    '---\ndescription: review code\nargument-hint: "[files]"\nallowed-tools: read, grep\n---\nReview.\n',
  );
  writeFileSync(
    join(dir, "agents", "reviewer.md"),
    "---\nname: reviewer\ndescription: reviews\ntools: read, grep\nmodel: opus\n---\nYou review.\n",
  );
  writeFileSync(join(dir, "CLAUDE.md"), "dir memory\n");
  writeFileSync(join(root, "CLAUDE.md"), "project memory\n");
  writeFileSync(
    join(dir, "settings.json"),
    JSON.stringify({
      model: "opus",
      hooks: {
        PostToolUse: [
          { matcher: "^edit$", hooks: [{ type: "command", command: "fmt.sh", timeout: 30 }] },
        ],
      },
    }),
  );
  writeFileSync(
    join(dir, "settings.local.json"),
    JSON.stringify({
      hooks: { Stop: [{ hooks: [{ type: "prompt", prompt: "really done?" }] }] },
    }),
  );
  writeFileSync(join(root, ".mcp.json"), JSON.stringify({ mcpServers: { gh: { command: "gh" } } }));
  return dir;
};

test("read picks up every config surface of a project .claude dir", async () =>
  withTempDir(async (root) => {
    const dir = fixture(root);
    const config = await Effect.runPromise(ClaudeConfig.read(dir));

    expect(config.skills.length).toBe(1);
    expect(config.skills[0].name).toBe("pnpm");
    expect(config.skills[0].files[0].path).toBe("references/core.md");
    // Devin-style extras ride in metadata for a lossless write-back.
    expect((config.skills[0].metadata as Record<string, unknown>)["subagent"]).toBe(true);

    const ruleNames = config.rules.map((r) => r.name);
    expect(ruleNames).toEqual(["claude", "claude-project"]);
    expect(config.rules[0].body).toBe("dir memory");
    expect(config.rules[1].body).toBe("project memory");
    expect(config.rules.every((r) => r.alwaysApply && r.kind === "instructions")).toBe(true);

    expect(config.commands[0].name).toBe("review");
    expect(Option.getOrNull(config.commands[0].argumentHint)).toBe("[files]");
    expect(config.commands[0].allowedTools).toEqual(["read", "grep"]);

    expect(config.agents[0].name).toBe("reviewer");
    expect(config.agents[0].tools).toEqual(["read", "grep"]);
    expect(Option.getOrNull(config.agents[0].model)).toBe("opus");

    expect(config.hooks.length).toBe(2);
    expect(config.hooks[0].event).toBe("PostToolUse");
    expect(Option.getOrNull(config.hooks[0].matcher)).toBe("^edit$");
    expect(Option.getOrNull(config.hooks[0].timeoutSec)).toBe(30);
    expect(config.hooks[1].type).toBe("prompt");

    expect(config.mcpServers["gh"]).toEqual({ command: "gh" });
  }));

test("read tolerates bare docs and absent settings/mcp files", async () =>
  withTempDir(async (root) => {
    const dir = join(root, ".claude");
    mkdirSync(join(dir, "commands"), { recursive: true });
    mkdirSync(join(dir, "agents"), { recursive: true });
    writeFileSync(join(dir, "commands", "bare.md"), "no frontmatter\n");
    writeFileSync(join(dir, "agents", "noname.md"), "no frontmatter either\n");
    const config = await Effect.runPromise(ClaudeConfig.read(dir));
    expect(config.commands[0].name).toBe("bare");
    expect(Option.isNone(config.commands[0].description)).toBe(true);
    expect(Option.isNone(config.commands[0].argumentHint)).toBe(true);
    expect(config.commands[0].allowedTools).toEqual([]);
    expect(config.agents[0].name).toBe("noname");
    expect(Option.isNone(config.agents[0].model)).toBe(true);
    expect(config.hooks).toEqual([]);
    expect(config.mcpServers).toEqual({});
  }));

test("read on an empty dir yields an empty config", async () =>
  withTempDir(async (root) => {
    const dir = join(root, ".claude");
    mkdirSync(dir, { recursive: true });
    const config = await Effect.runPromise(ClaudeConfig.read(dir));
    expect(config.rules).toEqual([]);
    expect(config.hooks).toEqual([]);
    expect(config.mcpServers).toEqual({});
  }));

test("write creates skills, commands, agents, CLAUDE.md block, merged hooks and .mcp.json", async () =>
  withTempDir(async (root) => {
    const dir = join(root, ".claude");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "settings.json"), JSON.stringify({ model: "opus" }));
    const config = AgentConfig.make({
      skills: [ConfigSkill.make({ name: "s1", body: "skill body", metadata: null })],
      rules: [
        ConfigRule.make({ name: "style", body: "use bun", alwaysApply: true, metadata: null }),
      ],
      commands: [
        ConfigCommand.make({
          name: "go",
          body: "do it",
          description: Option.some("go cmd"),
          metadata: null,
        }),
        ConfigCommand.make({ name: "   ", body: "bad stem", metadata: null }),
      ],
      hooks: [
        ConfigHook.make({
          event: "PostToolUse",
          matcher: Option.some("^edit$"),
          command: Option.some("fmt.sh"),
          metadata: null,
        }),
      ],
      agents: [
        ConfigAgent.make({
          name: "helper",
          body: "you help",
          description: Option.some("helps"),
          metadata: null,
        }),
      ],
      mcpServers: { srv: { command: "srv" } },
      metadata: null,
    });

    const actions = await Effect.runPromise(ClaudeConfig.write(config, dir));
    expect(
      actions.every(
        (a) => a.action === "wrote" || a.action === "updated" || a.action === "skipped",
      ),
    ).toBe(true);
    expect(actions.some((a) => a.action === "skipped")).toBe(true);

    expect(readFileSync(join(dir, "skills", "s1", "SKILL.md"), "utf-8")).toContain("skill body");
    expect(readFileSync(join(dir, "commands", "go.md"), "utf-8")).toContain("description: go cmd");
    expect(readFileSync(join(dir, "agents", "helper.md"), "utf-8")).toContain("name: helper");

    const memory = readFileSync(join(dir, "CLAUDE.md"), "utf-8");
    expect(memory).toContain("<!-- sepia:rules -->");
    expect(memory).toContain("## style");
    expect(memory).toContain("use bun");

    const settings = JSON.parse(readFileSync(join(dir, "settings.json"), "utf-8"));
    expect(settings.model).toBe("opus");
    expect(settings.hooks.PostToolUse[0].matcher).toBe("^edit$");
    expect(settings.hooks.PostToolUse[0].hooks[0].command).toBe("fmt.sh");

    const mcp = JSON.parse(readFileSync(join(root, ".mcp.json"), "utf-8"));
    expect(mcp.mcpServers.srv).toEqual({ command: "srv" });

    // Re-writing the same config is a no-op for content-stable files.
    const again = await Effect.runPromise(ClaudeConfig.write(config, dir));
    expect(again.some((a) => a.action === "unchanged")).toBe(true);
    expect(
      JSON.parse(readFileSync(join(dir, "settings.json"), "utf-8")).hooks.PostToolUse[0].hooks
        .length,
    ).toBe(1);
  }));

test("read/write on a non-.claude dir uses <dir>/.mcp.json for MCP", async () =>
  withTempDir(async (root) => {
    const dir = join(root, "custom");
    const config = await Effect.runPromise(
      Effect.gen(function* () {
        const written = yield* ClaudeConfig.write(
          AgentConfig.make({ mcpServers: { s: { command: "s" } }, metadata: null }),
          dir,
        );
        return written;
      }),
    );
    expect(config.length).toBe(1);
    expect(readFileSync(join(dir, ".mcp.json"), "utf-8")).toContain('"s"');
    const back = await Effect.runPromise(ClaudeConfig.read(dir));
    expect(back.mcpServers["s"]).toEqual({ command: "s" });
    expect(back.rules).toEqual([]);
  }));

test("write round-trips through read", async () =>
  withTempDir(async (root) => {
    const source = fixture(join(root, "src"));
    const config = await Effect.runPromise(ClaudeConfig.read(source));
    const target = join(root, "dst", ".claude");
    mkdirSync(target, { recursive: true });
    await Effect.runPromise(ClaudeConfig.write(config, target));
    const back = await Effect.runPromise(ClaudeConfig.read(target));
    expect(back.skills[0].name).toBe("pnpm");
    expect(back.commands[0].name).toBe("review");
    expect(back.agents[0].name).toBe("reviewer");
    expect(back.hooks.length).toBeGreaterThan(0);
    // The src CLAUDE.md instructions rule lands inside dst's managed block.
    expect(back.rules.length).toBe(1);
    expect(back.rules[0].body).toContain("dir memory");
    expect(back.mcpServers["gh"]).toEqual({ command: "gh" });
  }));
