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
} from "../src/AgentConfig.js";
import * as CursorConfig from "../src/CursorConfig.js";

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-cursor-config-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const fixture = (dir: string): void => {
  mkdirSync(join(dir, "rules"), { recursive: true });
  mkdirSync(join(dir, "commands"), { recursive: true });
  mkdirSync(join(dir, "agents"), { recursive: true });
  mkdirSync(join(dir, "skills", "canvas"), { recursive: true });
  writeFileSync(
    join(dir, "rules", "bun.mdc"),
    '---\ndescription: Use Bun\nglobs: "*.ts, *.tsx"\nalwaysApply: false\n---\nUse bun.\n',
  );
  writeFileSync(join(dir, "rules", "style.md"), "# Style\n\nNo comments.\n");
  writeFileSync(join(dir, "commands", "plan.md"), "---\ndescription: plan it\n---\nPlan.\n");
  writeFileSync(
    join(dir, "agents", "debugger.md"),
    "---\nname: debugger\ndescription: debugs\n---\nYou debug.\n",
  );
  writeFileSync(
    join(dir, "skills", "canvas", "SKILL.md"),
    "---\nname: canvas\ndescription: draw\n---\nDraw.\n",
  );
  writeFileSync(
    join(dir, "hooks.json"),
    JSON.stringify({
      version: 1,
      hooks: {
        afterFileEdit: [{ command: ".cursor/hooks/fmt.sh", matcher: "Write|TabWrite" }],
        beforeSubmitPrompt: [
          { type: "prompt", prompt: "no secrets?", failClosed: true, timeout: 10 },
        ],
        stop: [{ command: "loop.sh", loop_limit: 3 }],
      },
    }),
  );
  writeFileSync(
    join(dir, "mcp.json"),
    JSON.stringify({ mcpServers: { pg: { command: "pg-mcp" } } }),
  );
};

test("read picks up rules (mdc + md), commands, agents, skills, hooks and mcp", async () =>
  withTempDir(async (dir) => {
    fixture(dir);
    const config = await Effect.runPromise(CursorConfig.read(dir));

    expect(config.rules.length).toBe(2);
    const bun = config.rules.find((r) => r.name === "bun");
    expect(bun?.globs).toEqual(["*.ts", "*.tsx"]);
    expect(bun?.alwaysApply).toBe(false);
    expect(Option.getOrNull(bun?.description ?? Option.none())).toBe("Use Bun");
    const style = config.rules.find((r) => r.name === "style");
    expect(style?.body).toContain("No comments");

    expect(config.commands[0].name).toBe("plan");
    expect(config.agents[0].name).toBe("debugger");
    expect(config.skills[0].name).toBe("canvas");

    expect(config.hooks.length).toBe(3);
    const edit = config.hooks.find((h) => h.event === "AfterFileEdit");
    expect(edit).toBeDefined();
    expect(Option.getOrNull(edit?.matcher ?? Option.none())).toBe("Write|TabWrite");
    const prompt = config.hooks.find((h) => h.event === "UserPromptSubmit");
    expect(prompt?.type).toBe("prompt");
    expect(Option.getOrNull(prompt?.failClosed ?? Option.none())).toBe(true);
    const stop = config.hooks.find((h) => h.event === "Stop");
    expect(Option.getOrNull(stop?.loopLimit ?? Option.none())).toBe(3);

    expect(config.mcpServers["pg"]).toEqual({ command: "pg-mcp" });
  }));

test("write emits .md rules, merges hooks.json camelCase and mcp.json", async () =>
  withTempDir(async (dir) => {
    writeFileSync(
      join(dir, "hooks.json"),
      JSON.stringify({ version: 1, hooks: { stop: [{ command: "keep.sh" }] } }),
    );
    writeFileSync(
      join(dir, "mcp.json"),
      JSON.stringify({ mcpServers: { keep: { command: "keep" } }, disabled: true }),
    );
    const config = AgentConfig.make({
      rules: [
        ConfigRule.make({
          name: "style",
          body: "no comments",
          globs: ["*.ts"],
          alwaysApply: true,
          metadata: null,
        }),
        ConfigRule.make({
          name: "claude",
          body: "memory",
          kind: "instructions",
          metadata: null,
        }),
      ],
      commands: [
        ConfigCommand.make({ name: "go", body: "do", metadata: null }),
        ConfigCommand.make({ name: "   ", body: "bad stem", metadata: null }),
      ],
      skills: [ConfigSkill.make({ name: "s", body: "b", metadata: null })],
      agents: [ConfigAgent.make({ name: "a", body: "b", metadata: null })],
      hooks: [
        ConfigHook.make({
          event: "PostToolUse",
          matcher: Option.some("Shell"),
          command: Option.some("check.sh"),
          metadata: null,
        }),
        ConfigHook.make({
          event: "Stop",
          command: Option.some("keep.sh"),
          metadata: null,
        }),
        ConfigHook.make({ event: "SessionStart", type: "prompt", metadata: "odd" }),
      ],
      mcpServers: { add: { command: "add" } },
      metadata: null,
    });

    const actions = await Effect.runPromise(CursorConfig.write(config, dir));
    expect(actions.length).toBeGreaterThan(0);

    const styleRule = readFileSync(join(dir, "rules", "style.md"), "utf-8");
    expect(styleRule).toContain("alwaysApply: true");
    // `*` opens a YAML alias, so glob strings always render quoted.
    expect(styleRule).toContain('globs: "*.ts"');
    // instructions-kind rules land in rules/ as alwaysApply.
    const claudeRule = readFileSync(join(dir, "rules", "claude.md"), "utf-8");
    expect(claudeRule).toContain("alwaysApply: true");

    const hooks = JSON.parse(readFileSync(join(dir, "hooks.json"), "utf-8"));
    expect(hooks.version).toBe(1);
    // Pre-existing hook survives; PostToolUse landed under its cursor name.
    expect(hooks.hooks.stop).toEqual([{ command: "keep.sh" }]);
    expect(hooks.hooks.postToolUse).toEqual([{ command: "check.sh", matcher: "Shell" }]);

    const mcp = JSON.parse(readFileSync(join(dir, "mcp.json"), "utf-8"));
    expect(mcp.disabled).toBe(true);
    expect(mcp.mcpServers).toEqual({ keep: { command: "keep" }, add: { command: "add" } });
  }));

test("read tolerates malformed hooks.json and agent name fallback", async () =>
  withTempDir(async (dir) => {
    mkdirSync(join(dir, "agents"), { recursive: true });
    writeFileSync(join(dir, "agents", "noname.md"), "---\ndescription: d\n---\nbody\n");
    writeFileSync(join(dir, "hooks.json"), JSON.stringify({ hooks: "nope" }));
    writeFileSync(join(dir, "mcp.json"), "not json");
    const config = await Effect.runPromise(CursorConfig.read(dir));
    expect(config.agents[0].name).toBe("noname");
    expect(config.hooks).toEqual([]);
    expect(config.mcpServers).toEqual({});

    // Entries that aren't objects, and events that aren't arrays, are skipped.
    writeFileSync(
      join(dir, "hooks.json"),
      JSON.stringify({ hooks: { stop: ["x", { command: "a.sh" }], bad: "v" } }),
    );
    const again = await Effect.runPromise(CursorConfig.read(dir));
    expect(again.hooks.length).toBe(1);
    expect(again.hooks[0].event).toBe("Stop");

    // A PascalCase wire name needs no canonicalization — sourceEvent absent.
    writeFileSync(
      join(dir, "hooks.json"),
      JSON.stringify({ hooks: { Stop: [{ command: "b.sh" }] } }),
    );
    const direct = await Effect.runPromise(CursorConfig.read(dir));
    expect((direct.hooks[0].metadata as Record<string, unknown>)["sourceEvent"]).toBeUndefined();
  }));

test("write round-trips through read", async () =>
  withTempDir(async (dir) => {
    fixture(dir);
    const config = await Effect.runPromise(CursorConfig.read(dir));
    const target = join(dir, "out");
    await Effect.runPromise(CursorConfig.write(config, target));
    const back = await Effect.runPromise(CursorConfig.read(target));
    expect(back.rules.map((r) => r.name).sort()).toEqual(["bun", "style"]);
    expect(back.rules.find((r) => r.name === "bun")?.globs).toEqual(["*.ts", "*.tsx"]);
    expect(back.commands[0].name).toBe("plan");
    expect(back.agents[0].name).toBe("debugger");
    expect(back.skills[0].name).toBe("canvas");
    expect(back.hooks.length).toBe(3);
    expect(back.mcpServers["pg"]).toEqual({ command: "pg-mcp" });
  }));
