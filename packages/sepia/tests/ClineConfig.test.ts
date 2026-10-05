import { Effect } from "effect";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vite-plus/test";
import {
  AgentConfig,
  ConfigCommand,
  ConfigHook,
  ConfigRule,
  ConfigSkill,
} from "../src/AgentConfig.js";
import * as ClineConfig from "../src/ClineConfig.js";

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-cline-config-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

test("read treats a .clinerules file as one instructions rule", async () =>
  withTempDir(async (dir) => {
    writeFileSync(join(dir, ".clinerules"), "Always use bun.\n");
    const config = await Effect.runPromise(ClineConfig.read(dir));
    expect(config.rules.length).toBe(1);
    expect(config.rules[0].name).toBe("clinerules");
    expect(config.rules[0].kind).toBe("instructions");
    expect(config.rules[0].alwaysApply).toBe(true);
    expect(config.rules[0].body).toBe("Always use bun.");
  }));

test("read treats a .clinerules/ dir as rules plus workflows commands", async () =>
  withTempDir(async (dir) => {
    mkdirSync(join(dir, ".clinerules", "workflows"), { recursive: true });
    writeFileSync(join(dir, ".clinerules", "style.md"), "No comments.\n");
    writeFileSync(join(dir, ".clinerules", "docs.md"), "---\ndescription: doc rule\n---\nDocs.\n");
    writeFileSync(
      join(dir, ".clinerules", "workflows", "review.md"),
      "---\ndescription: review\n---\nReview the diff.\n",
    );
    writeFileSync(
      join(dir, "cline_mcp_settings.json"),
      JSON.stringify({ mcpServers: { fs: { command: "fs-mcp" } } }),
    );
    const config = await Effect.runPromise(ClineConfig.read(dir));
    expect(config.rules.map((r) => r.name).sort()).toEqual(["docs", "style"]);
    expect(config.rules.every((r) => r.alwaysApply)).toBe(true);
    expect(config.commands.length).toBe(1);
    expect(config.commands[0].name).toBe("review");
    expect(config.commands[0].body).toBe("Review the diff.\n");
    expect(config.mcpServers["fs"]).toEqual({ command: "fs-mcp" });
  }));

test("read on an empty workspace yields an empty config", async () =>
  withTempDir(async (dir) => {
    const config = await Effect.runPromise(ClineConfig.read(dir));
    expect(config.rules).toEqual([]);
    expect(config.commands).toEqual([]);
    expect(config.mcpServers).toEqual({});
  }));

test("write emits .clinerules/ files, workflows, merged mcp and skipped notes", async () =>
  withTempDir(async (dir) => {
    const config = AgentConfig.make({
      rules: [
        ConfigRule.make({ name: "style", body: "no comments", metadata: null }),
        ConfigRule.make({ name: "   ", body: "bad stem", metadata: null }),
      ],
      commands: [ConfigCommand.make({ name: "go", body: "do it", metadata: null })],
      skills: [ConfigSkill.make({ name: "s", metadata: null })],
      hooks: [ConfigHook.make({ event: "Stop", metadata: null })],
      mcpServers: { srv: { command: "srv" } },
      metadata: null,
    });
    const actions = await Effect.runPromise(ClineConfig.write(config, dir));
    expect(readFileSync(join(dir, ".clinerules", "style.md"), "utf-8")).toContain("no comments");
    expect(readFileSync(join(dir, ".clinerules", "workflows", "go.md"), "utf-8")).toContain(
      "do it",
    );
    const mcp = JSON.parse(readFileSync(join(dir, "cline_mcp_settings.json"), "utf-8"));
    expect(mcp.mcpServers.srv).toEqual({ command: "srv" });
    const skipped = actions.filter((a) => a.action === "skipped");
    expect(skipped.length).toBe(3); // bad stem, skills, hooks (agents list is empty → no note)
    expect(skipped.some((a) => a.detail?.includes("skills"))).toBe(true);
    expect(skipped.some((a) => a.detail?.includes("hooks"))).toBe(true);
  }));

test("write round-trips through read", async () =>
  withTempDir(async (dir) => {
    mkdirSync(join(dir, ".clinerules", "workflows"), { recursive: true });
    writeFileSync(join(dir, ".clinerules", "style.md"), "No comments.\n");
    writeFileSync(join(dir, ".clinerules", "workflows", "go.md"), "Do.\n");
    const config = await Effect.runPromise(ClineConfig.read(dir));
    const target = join(dir, "out");
    await Effect.runPromise(ClineConfig.write(config, target));
    const back = await Effect.runPromise(ClineConfig.read(target));
    expect(back.rules.map((r) => r.name)).toEqual(["style"]);
    expect(back.commands.map((c) => c.name)).toEqual(["go"]);
  }));
