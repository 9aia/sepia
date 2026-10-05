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
import * as DevinConfig from "../src/DevinConfig.js";

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-devin-config-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

const fixture = (dir: string): void => {
  mkdirSync(join(dir, "skills", "understand"), { recursive: true });
  mkdirSync(join(dir, "rules"), { recursive: true });
  mkdirSync(join(dir, "workflows"), { recursive: true });
  writeFileSync(
    join(dir, "skills", "understand", "SKILL.md"),
    '---\nname: understand\ndescription: explain a change\nargument-hint: "[branch|commit|pr|file]"\nsubagent: true\n---\nExplain.\n',
  );
  writeFileSync(
    join(dir, "rules", "general.md"),
    "---\ndescription: core rules\n---\n- Be modular.\n",
  );
  writeFileSync(
    join(dir, "workflows", "plan.md"),
    "---\ndescription: plan a TODO item\n---\nTriggered by /plan.\n",
  );
  writeFileSync(join(dir, "workflows", "bare.md"), "No frontmatter here.\n");
  writeFileSync(join(dir, "AGENTS.md"), "# Repo notes\n\nUse cargo.\n");
  writeFileSync(
    join(dir, "hooks.v1.json"),
    JSON.stringify({
      PostToolUse: [
        {
          matcher: "^edit$",
          hooks: [{ type: "command", command: "python3 .devin/hooks/fmt.py", timeout: 120 }],
        },
      ],
    }),
  );
};

test("read picks up skills, rules, workflows, AGENTS.md and hooks.v1.json", async () =>
  withTempDir(async (dir) => {
    fixture(dir);
    const config = await Effect.runPromise(DevinConfig.read(dir));

    expect(config.skills[0].name).toBe("understand");
    const meta = config.skills[0].metadata as Record<string, unknown>;
    expect(meta["argument-hint"]).toBe("[branch|commit|pr|file]");
    expect(meta["subagent"]).toBe(true);

    expect(config.rules.map((r) => r.name).sort()).toEqual(["agents", "general"]);
    const agents = config.rules.find((r) => r.name === "agents");
    expect(agents?.kind).toBe("instructions");
    expect(agents?.body).toContain("Use cargo");
    const general = config.rules.find((r) => r.name === "general");
    expect(general?.alwaysApply).toBe(true);
    expect(general?.kind).toBe("rule");
    expect(Option.getOrNull(general?.description ?? Option.none())).toBe("core rules");

    expect(config.commands.map((c) => c.name)).toEqual(["bare", "plan"]);
    const bare = config.commands.find((c) => c.name === "bare");
    expect(Option.isNone(bare?.description ?? Option.none())).toBe(true);
    expect(Option.getOrNull(config.commands[1].description)).toBe("plan a TODO item");

    expect(config.hooks.length).toBe(1);
    expect(config.hooks[0].event).toBe("PostToolUse");
    expect(Option.getOrNull(config.hooks[0].command)).toBe("python3 .devin/hooks/fmt.py");
    expect(Option.getOrNull(config.hooks[0].timeoutSec)).toBe(120);
  }));

test("read on an empty dir yields an empty config", async () =>
  withTempDir(async (dir) => {
    const config = await Effect.runPromise(DevinConfig.read(dir));
    expect(config.rules).toEqual([]);
    expect(config.hooks).toEqual([]);
    expect(config.skills).toEqual([]);
  }));

test("write emits skills/rules/workflows, merges AGENTS.md and hooks.v1.json", async () =>
  withTempDir(async (dir) => {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "AGENTS.md"), "# existing\n");
    writeFileSync(
      join(dir, "hooks.v1.json"),
      JSON.stringify({ Stop: [{ hooks: [{ type: "command", command: "done.sh" }] }] }),
    );
    const config = AgentConfig.make({
      skills: [ConfigSkill.make({ name: "s", body: "b", metadata: null })],
      rules: [
        ConfigRule.make({ name: "style", body: "r", metadata: null }),
        ConfigRule.make({
          name: "claude",
          body: "memory",
          kind: "instructions",
          metadata: null,
        }),
      ],
      commands: [
        ConfigCommand.make({ name: "w", body: "wf", metadata: null }),
        ConfigCommand.make({ name: "   ", body: "bad stem", metadata: null }),
      ],
      hooks: [
        ConfigHook.make({
          event: "PostToolUse",
          command: Option.some("fmt.sh"),
          metadata: null,
        }),
      ],
      agents: [ConfigAgent.make({ name: "sub", body: "x", metadata: null })],
      mcpServers: { srv: { command: "srv" } },
      metadata: null,
    });
    const actions = await Effect.runPromise(DevinConfig.write(config, dir));

    expect(readFileSync(join(dir, "skills", "s", "SKILL.md"), "utf-8")).toContain("b");
    expect(readFileSync(join(dir, "rules", "style.md"), "utf-8")).toContain("r");
    expect(readFileSync(join(dir, "workflows", "w.md"), "utf-8")).toContain("wf");

    const agentsMd = readFileSync(join(dir, "AGENTS.md"), "utf-8");
    expect(agentsMd).toContain("# existing");
    expect(agentsMd).toContain("<!-- sepia:rules -->");
    expect(agentsMd).toContain("## claude");

    const hooks = JSON.parse(readFileSync(join(dir, "hooks.v1.json"), "utf-8"));
    expect(hooks.Stop).toEqual([{ hooks: [{ type: "command", command: "done.sh" }] }]);
    expect(hooks.PostToolUse).toEqual([{ hooks: [{ type: "command", command: "fmt.sh" }] }]);

    const skipped = actions.filter((a) => a.action === "skipped");
    expect(skipped.length).toBe(3); // bad stem + subagents + mcpServers
  }));

test("write round-trips through read", async () =>
  withTempDir(async (dir) => {
    fixture(dir);
    const config = await Effect.runPromise(DevinConfig.read(dir));
    const target = join(dir, "out");
    await Effect.runPromise(DevinConfig.write(config, target));
    const back = await Effect.runPromise(DevinConfig.read(target));
    expect(back.skills[0].name).toBe("understand");
    expect(back.commands.map((c) => c.name)).toEqual(["bare", "plan"]);
    expect(back.rules.some((r) => r.name === "general")).toBe(true);
    expect(back.hooks.length).toBe(1);
  }));
