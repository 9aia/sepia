import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import { Effect, Layer, Option } from "effect";
import { homedir } from "node:os";
import {
  AgentConfig,
  ConfigAgent,
  ConfigCommand,
  ConfigRule,
  hooksFromClaudeJson,
  isObject,
  mergeClaudeHooks,
  mergeMcpServers,
  readFileIfExists,
  readJsonIfExists,
  readMarkdownDir,
  readSkillsDir,
  rulesToMemoryFile,
  safeFileStem,
  stringList,
  writeFileAction,
  writeSkillsDir,
  type ConfigWriteAction,
  type MarkdownDoc,
} from "sepia-core";
import { StorageError } from "sepia-core";
import { Frontmatter } from "sepia-core";

/**
 * Claude Code config store — `dir` is a `.claude` directory: `~/.claude`
 * (user scope) or `<repo>/.claude` (project scope).
 *
 * Read:
 * - `CLAUDE.md` inside the dir (memory → `kind: "instructions"`), plus the
 *   parent `CLAUDE.md` for a project `.claude` dir (`claude-project`).
 * - `skills/<name>/SKILL.md` + sibling files, `commands/<name>.md`,
 *   `agents/<name>.md`.
 * - `hooks` from `settings.json` and `settings.local.json` (each hook's
 *   `metadata.source` records which).
 * - `mcpServers` from `~/.claude.json` (user dir) or the sibling
 *   `.mcp.json` (project dir).
 *
 * Write: skills/commands/agents map one-to-one; hooks merge into
 * `settings.json` preserving other keys; rules collapse into a managed
 * `sepia:rules` block inside `CLAUDE.md` (Claude has no rules directory);
 * `mcpServers` merge into `~/.claude.json` or the sibling `.mcp.json`.
 */

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const isUserDir = (dir: string): boolean => dir === `${homedir()}/.claude`;
const isProjectDir = (dir: string): boolean => dir.endsWith("/.claude") && !isUserDir(dir);
const projectRoot = (dir: string): string => dir.slice(0, dir.length - "/.claude".length);

const extraFrontmatter = (meta: unknown): Record<string, unknown> => {
  if (!isObject(meta)) return {};
  const { sourcePath: _p, source: _s, ...rest } = meta;
  return rest;
};

const docToCommand = (doc: MarkdownDoc): ConfigCommand => {
  const {
    description,
    "argument-hint": argumentHint,
    "allowed-tools": allowedTools,
    model,
    ...rest
  } = doc.attributes;
  return ConfigCommand.make({
    name: doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    argumentHint: Option.fromNullable(typeof argumentHint === "string" ? argumentHint : undefined),
    allowedTools: stringList(allowedTools),
    model: Option.fromNullable(typeof model === "string" ? model : undefined),
    metadata: { ...rest, sourcePath: doc.path },
  });
};

const docToAgent = (doc: MarkdownDoc): ConfigAgent => {
  const { name, description, tools, model, ...rest } = doc.attributes;
  return ConfigAgent.make({
    name: typeof name === "string" && name !== "" ? name : doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    tools: stringList(tools),
    model: Option.fromNullable(typeof model === "string" ? model : undefined),
    metadata: { ...rest, sourcePath: doc.path },
  });
};

const instructionsRule = (name: string, path: string, body: string): ConfigRule =>
  ConfigRule.make({
    name,
    body: body.trimEnd(),
    alwaysApply: true,
    kind: "instructions",
    metadata: { sourcePath: path },
  });

const program = (dir: string) =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;

    const rules: ConfigRule[] = [];
    const memory = yield* readFileIfExists(fs, `${dir}/CLAUDE.md`);
    if (memory !== undefined) {
      rules.push(instructionsRule("claude", `${dir}/CLAUDE.md`, memory));
    }
    if (isProjectDir(dir)) {
      const projectMemory = yield* readFileIfExists(fs, `${projectRoot(dir)}/CLAUDE.md`);
      if (projectMemory !== undefined) {
        rules.push(
          instructionsRule("claude-project", `${projectRoot(dir)}/CLAUDE.md`, projectMemory),
        );
      }
    }

    const skills = yield* readSkillsDir(fs, `${dir}/skills`);
    const commands = (yield* readMarkdownDir(fs, `${dir}/commands`, [".md"])).map(docToCommand);
    const agents = (yield* readMarkdownDir(fs, `${dir}/agents`, [".md"])).map(docToAgent);

    const settings = yield* readJsonIfExists(fs, `${dir}/settings.json`);
    const localSettings = yield* readJsonIfExists(fs, `${dir}/settings.local.json`);
    const hooks = [
      ...(settings === undefined ? [] : hooksFromClaudeJson(settings, "settings.json")),
      ...(localSettings === undefined
        ? []
        : hooksFromClaudeJson(localSettings, "settings.local.json")),
    ];

    const mcpPath = isUserDir(dir)
      ? `${homedir()}/.claude.json`
      : isProjectDir(dir)
        ? `${projectRoot(dir)}/.mcp.json`
        : `${dir}/.mcp.json`;
    const mcpFile = yield* readJsonIfExists(fs, mcpPath);
    const mcpServers =
      mcpFile !== undefined && isObject(mcpFile["mcpServers"])
        ? (mcpFile["mcpServers"] as Record<string, unknown>)
        : {};

    return AgentConfig.make({
      skills: [...skills],
      rules,
      commands: [...commands],
      hooks: [...hooks],
      agents: [...agents],
      mcpServers,
      metadata: { source: "claude", dir },
    });
  });

/** Read the Claude Code config under `.claude` dir `dir` into the config IR. */
export const read = (dir: string): Effect.Effect<AgentConfig, StorageError> =>
  program(dir).pipe(Effect.provide(fsLayer));

const commandDoc = (command: ConfigCommand): string =>
  Frontmatter.render(
    {
      ...(Option.isSome(command.description) ? { description: command.description.value } : {}),
      ...(Option.isSome(command.argumentHint)
        ? { "argument-hint": command.argumentHint.value }
        : {}),
      ...(command.allowedTools.length > 0 ? { "allowed-tools": command.allowedTools } : {}),
      ...(Option.isSome(command.model) ? { model: command.model.value } : {}),
      ...extraFrontmatter(command.metadata),
    },
    command.body,
  );

const agentDoc = (agent: ConfigAgent): string =>
  Frontmatter.render(
    {
      name: agent.name,
      ...(Option.isSome(agent.description) ? { description: agent.description.value } : {}),
      ...(agent.tools.length > 0 ? { tools: agent.tools.join(", ") } : {}),
      ...(Option.isSome(agent.model) ? { model: agent.model.value } : {}),
      ...extraFrontmatter(agent.metadata),
    },
    agent.body,
  );

const writeDocs = <A extends { readonly name: string }>(
  fs: Fs.FileSystem,
  dir: string,
  items: ReadonlyArray<A>,
  render: (item: A) => string,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.forEach(items, (item) => {
    const stem = safeFileStem(item.name);
    if (stem === null) {
      return Effect.succeed<ConfigWriteAction>({
        path: dir,
        action: "skipped" as const,
        detail: `${JSON.stringify(item.name)} has no usable file stem`,
      });
    }
    return writeFileAction(fs, `${dir}/${stem}.md`, render(item));
  });

/** Write `config` into the `.claude` dir `dir`; returns per-file actions. */
export const write = (
  config: AgentConfig,
  dir: string,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const actions: ConfigWriteAction[] = [];

    actions.push(...(yield* writeSkillsDir(fs, `${dir}/skills`, config.skills)));
    actions.push(...(yield* writeDocs(fs, `${dir}/commands`, config.commands, commandDoc)));
    actions.push(...(yield* writeDocs(fs, `${dir}/agents`, config.agents, agentDoc)));

    if (config.rules.length > 0) {
      const path = `${dir}/CLAUDE.md`;
      const existing = yield* readFileIfExists(fs, path);
      const merged = rulesToMemoryFile(existing, config.rules);
      if (merged !== undefined) actions.push(yield* writeFileAction(fs, path, merged));
    }

    if (config.hooks.length > 0) {
      const path = `${dir}/settings.json`;
      const settings = (yield* readJsonIfExists(fs, path)) ?? {};
      const merged = mergeClaudeHooks(settings, config.hooks);
      const text = `${JSON.stringify(merged, null, 2)}\n`;
      const existing = yield* readFileIfExists(fs, path);
      if (existing !== text) {
        actions.push(yield* writeFileAction(fs, path, text));
      } else {
        actions.push({ path, action: "unchanged" as const });
      }
    }

    const mcpPath = isUserDir(dir)
      ? `${homedir()}/.claude.json`
      : isProjectDir(dir)
        ? `${projectRoot(dir)}/.mcp.json`
        : `${dir}/.mcp.json`;
    const mcp = mergeMcpServers(yield* readJsonIfExists(fs, mcpPath), config.mcpServers);
    if (mcp !== undefined) {
      actions.push(yield* writeFileAction(fs, mcpPath, `${JSON.stringify(mcp, null, 2)}\n`));
    }

    return actions;
  }).pipe(Effect.provide(fsLayer));
