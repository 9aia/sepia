import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import { Effect, Layer, Option } from "effect";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vite-plus/test";
import {
  AgentConfig,
  canonicalHookEvent,
  claudeHookEvent,
  ConfigAgent,
  ConfigCommand,
  ConfigHook,
  ConfigRule,
  ConfigSkill,
  configFromJson,
  configToJson,
  cursorHookEvent,
  hooksFromClaudeJson,
  listDirIfExists,
  mergeClaudeHooks,
  mergeHookEvents,
  mergeMcpServers,
  readFileIfExists,
  readJsonIfExists,
  readMarkdownDir,
  readSkillsDir,
  RULES_BLOCK_BEGIN,
  RULES_BLOCK_END,
  rulesToMemoryFile,
  safeFileStem,
  skillAttributes,
  stringList,
  writeFileAction,
  writeSkillsDir,
} from "../src/AgentConfig.js";

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);
const run = <A, E>(effect: Effect.Effect<A, E, Fs.FileSystem>) =>
  Effect.runPromise(effect.pipe(Effect.provide(fsLayer)));

const withTempDir = async <T>(fn: (dir: string) => T | Promise<T>): Promise<T> => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-config-"));
  try {
    return await fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

// ---------------------------------------------------------------------------
// IR + JSON
// ---------------------------------------------------------------------------

test("configToJson/configFromJson round-trips every collection", () => {
  const config = AgentConfig.make({
    skills: [
      ConfigSkill.make({
        name: "pnpm",
        description: Option.some("use pnpm"),
        body: "body",
        files: [{ path: "references/core.md", content: "ref" }],
        metadata: { "argument-hint": "[x]", sourcePath: "/s" },
      }),
    ],
    rules: [
      ConfigRule.make({
        name: "style",
        body: "rule body",
        globs: ["*.ts"],
        alwaysApply: false,
        metadata: null,
      }),
      ConfigRule.make({
        name: "claude",
        body: "memory",
        alwaysApply: true,
        kind: "instructions",
        metadata: null,
      }),
    ],
    commands: [
      ConfigCommand.make({
        name: "review",
        body: "review it",
        argumentHint: Option.some("[files]"),
        allowedTools: ["read", "grep"],
        metadata: null,
      }),
    ],
    hooks: [
      ConfigHook.make({
        event: "PostToolUse",
        matcher: Option.some("^edit$"),
        command: Option.some("sh hook.sh"),
        timeoutSec: Option.some(30),
        metadata: { source: "settings.json" },
      }),
    ],
    agents: [
      ConfigAgent.make({
        name: "reviewer",
        description: Option.some("reviews"),
        body: "you review",
        tools: ["read"],
        metadata: null,
      }),
    ],
    mcpServers: { github: { command: "mcp-github", args: ["--x"] } },
    metadata: null,
  });
  const json = configToJson(config);
  expect(json["version"]).toBe(1);
  const back = configFromJson(json);
  expect(back.skills[0].name).toBe("pnpm");
  expect(back.skills[0].files[0]).toEqual({ path: "references/core.md", content: "ref" });
  expect(Option.getOrNull(back.rules[0].description)).toBeNull();
  expect(back.rules[1].kind).toBe("instructions");
  expect(back.commands[0].allowedTools).toEqual(["read", "grep"]);
  expect(back.hooks[0].event).toBe("PostToolUse");
  expect(back.agents[0].tools).toEqual(["read"]);
  expect(back.mcpServers["github"]).toEqual({ command: "mcp-github", args: ["--x"] });
});

test("configFromJson applies every default on a minimal payload", () => {
  const back = configFromJson({ version: 1, skills: [{ name: "x" }] });
  expect(back.skills[0].body).toBe("");
  expect(back.skills[0].files).toEqual([]);
  expect(Option.isNone(back.skills[0].description)).toBe(true);
  expect(back.rules).toEqual([]);
  expect(back.mcpServers).toEqual({});
});

test("configFromJson fills wire defaults on sparse items and an empty doc", () => {
  const back = configFromJson({
    rules: [{ name: "r" }],
    commands: [{ name: "c" }],
    hooks: [{ event: "Stop" }],
    agents: [{ name: "a" }],
  });
  expect(back.commands[0].allowedTools).toEqual([]);
  expect(back.hooks[0].type).toBe("command");
  expect(back.rules[0].kind).toBe("rule");
  expect(back.agents[0].tools).toEqual([]);
  const empty = configFromJson({});
  expect(empty.skills).toEqual([]);
  expect(empty.mcpServers).toEqual({});
});

test("class constructors fill defaults for sparse makes", () => {
  const skill = ConfigSkill.make({ name: "x", metadata: null });
  expect(skill.body).toBe("");
  expect(skill.files).toEqual([]);
  const rule = ConfigRule.make({ name: "r", metadata: null });
  expect(rule.globs).toEqual([]);
  expect(rule.kind).toBe("rule");
  const hook = ConfigHook.make({ event: "Stop", metadata: null });
  expect(hook.type).toBe("command");
  const empty = AgentConfig.make({ metadata: null });
  expect(empty.skills).toEqual([]);
  expect(empty.mcpServers).toEqual({});
});

test("configFromJson rejects a non-config payload", () => {
  expect(() => configFromJson({ version: 1, skills: "nope" })).toThrow();
});

// ---------------------------------------------------------------------------
// Hook event names
// ---------------------------------------------------------------------------

test("hook event names map between canonical, cursor and claude", () => {
  expect(canonicalHookEvent("beforeSubmitPrompt")).toBe("UserPromptSubmit");
  expect(canonicalHookEvent("preToolUse")).toBe("PreToolUse");
  expect(canonicalHookEvent("sessionStart")).toBe("SessionStart");
  expect(canonicalHookEvent("UserPromptSubmit")).toBe("UserPromptSubmit");
  expect(canonicalHookEvent("beforeShellExecution")).toBe("BeforeShellExecution");
  expect(cursorHookEvent("PreToolUse")).toBe("preToolUse");
  expect(cursorHookEvent("UserPromptSubmit")).toBe("beforeSubmitPrompt");
  expect(cursorHookEvent("CustomEvent")).toBe("customEvent");
  expect(claudeHookEvent("PreToolUse")).toBe("PreToolUse");
  expect(claudeHookEvent("customEvent")).toBe("CustomEvent");
});

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

test("safeFileStem strips separators, dots and extensions", () => {
  expect(safeFileStem("name")).toBe("name");
  expect(safeFileStem("a/b\\c")).toBe("a-b-c");
  expect(safeFileStem(".hidden")).toBe("hidden");
  expect(safeFileStem("rule.md")).toBe("rule");
  expect(safeFileStem("two words")).toBe("two-words");
  expect(safeFileStem("...")).toBeNull();
  expect(safeFileStem("")).toBeNull();
});

test("stringList accepts comma strings and lists", () => {
  expect(stringList("*.ts, *.tsx")).toEqual(["*.ts", "*.tsx"]);
  expect(stringList(["a", "b"])).toEqual(["a", "b"]);
  expect(stringList(undefined)).toEqual([]);
  expect(stringList("")).toEqual([]);
  expect(stringList(3)).toEqual([]);
});

// ---------------------------------------------------------------------------
// Claude/Devin hook schema
// ---------------------------------------------------------------------------

test("hooksFromClaudeJson flattens matcher groups", () => {
  const settings = {
    permissions: { allow: ["x"] },
    hooks: {
      PostToolUse: [
        {
          matcher: "^edit$",
          hooks: [
            { type: "command", command: "fmt.sh", timeout: 30 },
            { type: "prompt", prompt: "check it" },
          ],
        },
      ],
      Stop: [{ hooks: [{ type: "command", command: "done.sh" }] }],
      Bad: ["not-a-group", { hooks: "nope" }, { hooks: [42] }],
    },
  };
  const hooks = hooksFromClaudeJson(settings, "settings.json");
  expect(hooks.length).toBe(3);
  expect(hooks[0].event).toBe("PostToolUse");
  expect(Option.getOrNull(hooks[0].matcher)).toBe("^edit$");
  expect(Option.getOrNull(hooks[0].command)).toBe("fmt.sh");
  expect(Option.getOrNull(hooks[0].timeoutSec)).toBe(30);
  expect(hooks[1].type).toBe("prompt");
  expect(Option.getOrNull(hooks[1].prompt)).toBe("check it");
  expect(Option.isNone(hooks[2].matcher)).toBe(true);
  expect(hooksFromClaudeJson({}, "x")).toEqual([]);
});

test("hooksFromClaudeJson accepts Devin's bare event map", () => {
  // hooks.v1.json is the event map itself — no wrapping `hooks` key.
  const bare = {
    PostToolUse: [{ matcher: "^edit$", hooks: [{ type: "command", command: "fmt.py" }] }],
  };
  const hooks = hooksFromClaudeJson(bare, "hooks.v1.json");
  expect(hooks.length).toBe(1);
  expect(hooks[0].event).toBe("PostToolUse");
  expect((hooks[0].metadata as Record<string, unknown>)["source"]).toBe("hooks.v1.json");
  // A settings-shaped object with a `hooks` key uses the nested map.
  const nested = { env: { X: "1" }, hooks: bare };
  expect(hooksFromClaudeJson(nested, "settings.json").length).toBe(1);
  // ...and a settings object without hooks finds none (non-array values skipped).
  expect(hooksFromClaudeJson({ env: { X: "1" }, model: "opus" }, "settings.json")).toEqual([]);
});

test("mergeHookEvents merges into a bare event map", () => {
  const merged = mergeHookEvents({ Stop: [{ hooks: [{ type: "command", command: "keep.sh" }] }] }, [
    ConfigHook.make({ event: "Stop", command: Option.some("keep.sh"), metadata: null }),
    ConfigHook.make({ event: "Stop", command: Option.some("new.sh"), metadata: null }),
  ]);
  const groups = merged["Stop"] as ReadonlyArray<Record<string, unknown>>;
  // keep.sh deduped against the existing group; new.sh appended alongside it.
  expect(groups.length).toBe(1);
  expect((groups[0]["hooks"] as unknown[]).length).toBe(2);
});

test("claude/cursor event names pass empty and unknown strings through", () => {
  expect(claudeHookEvent("")).toBe("");
  expect(cursorHookEvent("")).toBe("");
  expect(canonicalHookEvent("customWire")).toBe("CustomWire");
});

test("mergeClaudeHooks appends, dedupes and preserves other keys", () => {
  const existing = {
    model: "opus",
    hooks: {
      PostToolUse: [{ matcher: "^edit$", hooks: [{ type: "command", command: "old.sh" }] }],
    },
  };
  const merged = mergeClaudeHooks(existing, [
    ConfigHook.make({
      event: "PostToolUse",
      matcher: Option.some("^edit$"),
      command: Option.some("old.sh"),
      metadata: null,
    }),
    ConfigHook.make({
      event: "PostToolUse",
      matcher: Option.some("^edit$"),
      command: Option.some("new.sh"),
      timeoutSec: Option.some(5),
      metadata: null,
    }),
    ConfigHook.make({
      event: "Stop",
      command: Option.some("done.sh"),
      metadata: null,
    }),
  ]);
  expect(merged["model"]).toBe("opus");
  const groups = merged["hooks"] as Record<string, unknown>;
  const postGroups = groups["PostToolUse"] as ReadonlyArray<Record<string, unknown>>;
  expect(postGroups.length).toBe(1);
  const entries = postGroups[0]["hooks"] as ReadonlyArray<Record<string, unknown>>;
  // old.sh was identical → deduped; new.sh appended to the same matcher group.
  expect(entries.length).toBe(2);
  expect(entries[1]).toEqual({ type: "command", command: "new.sh", timeout: 5 });
  expect(groups["Stop"]).toEqual([{ hooks: [{ type: "command", command: "done.sh" }] }]);
});

// ---------------------------------------------------------------------------
// Memory-file rules block
// ---------------------------------------------------------------------------

test("rulesToMemoryFile appends a managed block to existing content", () => {
  const out = rulesToMemoryFile("# My memory\n\nkeep this\n", [
    ConfigRule.make({ name: "style", body: "use bun", metadata: null }),
    ConfigRule.make({
      name: "docs",
      body: "update docs",
      description: Option.some("doc rule"),
      globs: ["docs/**"],
      metadata: null,
    }),
  ]);
  expect(out).toContain("# My memory");
  expect(out).toContain(RULES_BLOCK_BEGIN);
  expect(out).toContain("## style");
  expect(out).toContain("> Applies to: docs/**");
  expect(out).toContain(RULES_BLOCK_END);
  // A second merge replaces the block without duplicating it.
  const again = rulesToMemoryFile(out, [
    ConfigRule.make({ name: "only", body: "new", metadata: null }),
  ]);
  expect(again).toContain("## only");
  expect(again?.match(/## style/g)).toBeNull();
  expect(again).toContain("# My memory");
});

test("rulesToMemoryFile returns undefined with no rules and no stale block", () => {
  expect(rulesToMemoryFile("# hi\n", [])).toBeUndefined();
  expect(rulesToMemoryFile(undefined, [])).toBeUndefined();
  const fresh = rulesToMemoryFile(undefined, [
    ConfigRule.make({ name: "x", body: "b", metadata: null }),
  ]);
  expect(fresh).toContain("## x");
});

test("rulesToMemoryFile empties a stale block when no rules remain", () => {
  const stale = `pre\n\n${RULES_BLOCK_BEGIN}\n\n## old\n\n${RULES_BLOCK_END}\npost\n`;
  const out = rulesToMemoryFile(stale, []);
  expect(out).toContain("pre");
  expect(out).toContain("post");
  expect(out).not.toContain("## old");
  expect(out).toContain(RULES_BLOCK_BEGIN);
});

// ---------------------------------------------------------------------------
// MCP merge
// ---------------------------------------------------------------------------

test("mergeMcpServers merges servers and preserves other keys", () => {
  expect(mergeMcpServers(undefined, {})).toBeUndefined();
  const merged = mergeMcpServers(
    { disabled: false, mcpServers: { old: { command: "old" } } },
    { new: { command: "new", args: ["a"] } },
  );
  expect(merged?.["disabled"]).toBe(false);
  expect(merged?.["mcpServers"]).toEqual({
    old: { command: "old" },
    new: { command: "new", args: ["a"] },
  });
});

// ---------------------------------------------------------------------------
// fs helpers
// ---------------------------------------------------------------------------

test("readFileIfExists/readJsonIfExists/listDirIfExists handle missing paths", async () =>
  withTempDir(async (dir) => {
    const fs = { fs: undefined };
    void fs;
    const read = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return {
          missing: yield* readFileIfExists(f, `${dir}/nope.txt`),
          json: yield* readJsonIfExists(f, `${dir}/nope.json`),
          badJson: yield* readJsonIfExists(f, `${dir}/bad.json`),
          entries: yield* listDirIfExists(f, `${dir}/no-dir`),
        };
      }),
    );
    writeFileSync(`${dir}/bad.json`, "not json");
    const read2 = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return {
          badJson: yield* readJsonIfExists(f, `${dir}/bad.json`),
          entries: yield* listDirIfExists(f, dir),
        };
      }),
    );
    expect(read.missing).toBeUndefined();
    expect(read.json).toBeUndefined();
    expect(read.entries).toEqual([]);
    expect(read2.badJson).toBeUndefined();
    expect(read2.entries).toContain("bad.json");
  }));

test("writeFileAction reports wrote/unchanged/updated and makes parents", async () =>
  withTempDir(async (dir) => {
    const path = `${dir}/deep/nested/file.txt`;
    const actions = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        const a = yield* writeFileAction(f, path, "v1");
        const b = yield* writeFileAction(f, path, "v1");
        const c = yield* writeFileAction(f, path, "v2");
        return [a, b, c];
      }),
    );
    expect(actions.map((a) => a.action)).toEqual(["wrote", "unchanged", "updated"]);
    expect(readFileSync(path, "utf-8")).toBe("v2");
  }));

test("readMarkdownDir filters extensions and parses frontmatter", async () =>
  withTempDir(async (dir) => {
    writeFileSync(`${dir}/b.mdc`, "---\ndescription: d\nglobs: *.ts\nalwaysApply: true\n---\nB\n");
    writeFileSync(`${dir}/a.md`, "plain\n");
    writeFileSync(`${dir}/c.txt`, "skip me\n");
    const fs = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return yield* readMarkdownDir(f, dir, [".mdc", ".md"]);
      }),
    );
    expect(fs.map((d) => d.fileName)).toEqual(["a.md", "b.mdc"]);
    expect(fs[0].body).toBe("plain\n");
    expect(fs[1].stem).toBe("b");
    expect(fs[1].attributes["alwaysApply"]).toBe(true);
  }));

test("readSkillsDir falls back to the dir name and tolerates odd metadata", async () =>
  withTempDir(async (dir) => {
    mkdirSync(`${dir}/skills/noname`, { recursive: true });
    writeFileSync(`${dir}/skills/noname/SKILL.md`, "---\ndescription: d\n---\nbody\n");
    const skills = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return yield* readSkillsDir(f, `${dir}/skills`);
      }),
    );
    expect(skills[0].name).toBe("noname");

    // skillAttributes emits name/description and drops provenance keys.
    const attrs = skillAttributes(
      ConfigSkill.make({
        name: "x",
        description: Option.some("d"),
        metadata: "not-an-object",
      }),
    );
    expect(attrs).toEqual({ name: "x", description: "d" });
  }));

test("writeSkillsDir skips absolute file paths", async () =>
  withTempDir(async (dir) => {
    const actions = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return yield* writeSkillsDir(f, `${dir}/s`, [
          ConfigSkill.make({
            name: "ok",
            files: [{ path: "/etc/passwd", content: "x" }],
            metadata: null,
          }),
        ]);
      }),
    );
    expect(actions.some((a) => a.action === "skipped")).toBe(true);
    expect(readFileSync(`${dir}/s/ok/SKILL.md`, "utf-8")).toContain("name: ok");
  }));

test("readSkillsDir/writeSkillsDir round-trip a skill with files", async () =>
  withTempDir(async (dir) => {
    const skillsDir = `${dir}/skills`;
    mkdirSync(`${skillsDir}/pnpm/references`, { recursive: true });
    writeFileSync(
      `${skillsDir}/pnpm/SKILL.md`,
      "---\nname: pnpm\ndescription: use pnpm\nextra: keep\n---\nUse pnpm.\n",
    );
    writeFileSync(`${skillsDir}/pnpm/references/core.md`, "ref doc");
    mkdirSync(`${skillsDir}/empty`, { recursive: true });
    const skills = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return yield* readSkillsDir(f, skillsDir);
      }),
    );
    expect(skills.length).toBe(1);
    expect(skills[0].name).toBe("pnpm");
    expect(Option.getOrNull(skills[0].description)).toBe("use pnpm");
    expect(skills[0].files).toEqual([{ path: "references/core.md", content: "ref doc" }]);

    const outDir = `${dir}/out-skills`;
    const actions = await run(
      Effect.gen(function* () {
        const f = yield* Fs.FileSystem;
        return yield* writeSkillsDir(f, outDir, [
          ...skills,
          ConfigSkill.make({ name: "   ", metadata: null }),
          ConfigSkill.make({
            name: "escape",
            files: [{ path: "../x", content: "no" }],
            metadata: null,
          }),
        ]);
      }),
    );
    const skillMd = readFileSync(`${outDir}/pnpm/SKILL.md`, "utf-8");
    expect(skillMd).toContain("name: pnpm");
    expect(skillMd).toContain("extra: keep");
    expect(readFileSync(`${outDir}/pnpm/references/core.md`, "utf-8")).toBe("ref doc");
    expect(actions.some((a) => a.action === "skipped")).toBe(true);
  }));
