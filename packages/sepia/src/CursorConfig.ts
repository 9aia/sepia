import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import { Effect, Layer, Option } from "effect";
import {
  AgentConfig,
  canonicalHookEvent,
  ConfigAgent,
  ConfigCommand,
  ConfigHook,
  ConfigRule,
  cursorHookEvent,
  isObject,
  mergeMcpServers,
  readJsonIfExists,
  readMarkdownDir,
  readSkillsDir,
  safeFileStem,
  stringList,
  writeFileAction,
  writeSkillsDir,
  type ConfigWriteAction,
  type MarkdownDoc,
} from "./AgentConfig.js";
import { StorageError } from "./Domain.js";
import * as Frontmatter from "./Frontmatter.js";

/**
 * Cursor config store — `dir` is a `.cursor` directory: `~/.cursor` (user
 * scope) or `<repo>/.cursor` (project scope).
 *
 * Read:
 * - `rules/*.{md,mdc}` — frontmatter `description`/`globs` (comma string or
 *   list)/`alwaysApply`; body is the rule text.
 * - `commands/<name>.md`, `agents/<name>.md`, `skills/<name>/SKILL.md`
 *   (the sibling `skills-cursor/` tree holds Cursor's built-ins and is not
 *   scanned).
 * - `hooks.json` `{version, hooks: {event: [entry]}}` — flat entries with
 *   `command`/`type`/`timeout`/`matcher`/`failClosed`/`loop_limit`; camelCase
 *   events normalize to the canonical PascalCase names.
 * - `mcp.json` `mcpServers`.
 *
 * Write is symmetric: rules land as `rules/<name>.md` (the modern `.md`
 * form — `.mdc` files still round-trip as `.md`), hooks merge into
 * `hooks.json` deduped per entry, MCP servers merge into `mcp.json`.
 */

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const extraFrontmatter = (meta: unknown): Record<string, unknown> => {
  if (!isObject(meta)) return {};
  const { sourcePath: _p, source: _s, sourceEvent: _e, ...rest } = meta;
  return rest;
};

const docToRule = (doc: MarkdownDoc): ConfigRule => {
  const { description, globs, alwaysApply, ...rest } = doc.attributes;
  return ConfigRule.make({
    name: doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    globs: stringList(globs),
    alwaysApply: alwaysApply === true,
    kind: "rule",
    metadata: { ...rest, sourcePath: doc.path },
  });
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

const hooksFromJson = (raw: Record<string, unknown>): ReadonlyArray<ConfigHook> => {
  const hooks = raw["hooks"];
  if (!isObject(hooks)) return [];
  const out: ConfigHook[] = [];
  for (const [wire, entries] of Object.entries(hooks)) {
    if (!Array.isArray(entries)) continue;
    for (const entry of entries) {
      if (!isObject(entry)) continue;
      const command = typeof entry["command"] === "string" ? entry["command"] : undefined;
      const prompt = typeof entry["prompt"] === "string" ? entry["prompt"] : undefined;
      const matcher = typeof entry["matcher"] === "string" ? entry["matcher"] : undefined;
      const timeout = typeof entry["timeout"] === "number" ? entry["timeout"] : undefined;
      const failClosed = typeof entry["failClosed"] === "boolean" ? entry["failClosed"] : undefined;
      const loopLimit = typeof entry["loop_limit"] === "number" ? entry["loop_limit"] : undefined;
      const canonical = canonicalHookEvent(wire);
      out.push(
        ConfigHook.make({
          event: canonical,
          matcher: Option.fromNullable(matcher),
          type: entry["type"] === "prompt" ? "prompt" : "command",
          command: Option.fromNullable(command),
          prompt: Option.fromNullable(prompt),
          timeoutSec: Option.fromNullable(timeout),
          failClosed: Option.fromNullable(failClosed),
          loopLimit: Option.fromNullable(loopLimit),
          metadata: { source: "hooks.json", ...(canonical === wire ? {} : { sourceEvent: wire }) },
        }),
      );
    }
  }
  return out;
};

const hookToCursorEntry = (hook: ConfigHook): Record<string, unknown> => ({
  ...(Option.isSome(hook.command) ? { command: hook.command.value } : {}),
  ...(hook.type === "prompt" ? { type: "prompt" } : {}),
  ...(Option.isSome(hook.prompt) ? { prompt: hook.prompt.value } : {}),
  ...(Option.isSome(hook.matcher) ? { matcher: hook.matcher.value } : {}),
  ...(Option.isSome(hook.timeoutSec) ? { timeout: hook.timeoutSec.value } : {}),
  ...(Option.isSome(hook.failClosed) ? { failClosed: hook.failClosed.value } : {}),
  ...(Option.isSome(hook.loopLimit) ? { loop_limit: hook.loopLimit.value } : {}),
});

const mergeCursorHooks = (
  raw: Record<string, unknown>,
  hooks: ReadonlyArray<ConfigHook>,
): Record<string, unknown> => {
  const existing = isObject(raw["hooks"]) ? (raw["hooks"] as Record<string, unknown>) : {};
  const merged: Record<string, unknown> = { ...existing };
  for (const hook of hooks) {
    const event = cursorHookEvent(hook.event);
    const entries = Array.isArray(merged[event]) ? [...(merged[event] as unknown[])] : [];
    const entry = hookToCursorEntry(hook);
    if (!entries.some((e) => JSON.stringify(e) === JSON.stringify(entry))) {
      entries.push(entry);
    }
    merged[event] = entries;
  }
  return { ...raw, version: 1, hooks: merged };
};

const program = (dir: string) =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;

    const rules = (yield* readMarkdownDir(fs, `${dir}/rules`, [".mdc", ".md"])).map(docToRule);
    const commands = (yield* readMarkdownDir(fs, `${dir}/commands`, [".md"])).map(docToCommand);
    const agents = (yield* readMarkdownDir(fs, `${dir}/agents`, [".md"])).map(docToAgent);
    const skills = yield* readSkillsDir(fs, `${dir}/skills`);

    const hooksJson = yield* readJsonIfExists(fs, `${dir}/hooks.json`);
    const hooks = hooksJson === undefined ? [] : hooksFromJson(hooksJson);

    const mcpJson = yield* readJsonIfExists(fs, `${dir}/mcp.json`);
    const mcpServers =
      mcpJson !== undefined && isObject(mcpJson["mcpServers"])
        ? (mcpJson["mcpServers"] as Record<string, unknown>)
        : {};

    return AgentConfig.make({
      skills: [...skills],
      rules: [...rules],
      commands: [...commands],
      hooks: [...hooks],
      agents: [...agents],
      mcpServers,
      metadata: { source: "cursor", dir },
    });
  });

/** Read the Cursor config under `.cursor` dir `dir` into the config IR. */
export const read = (dir: string): Effect.Effect<AgentConfig, StorageError> =>
  program(dir).pipe(Effect.provide(fsLayer));

const ruleDoc = (rule: ConfigRule): string =>
  Frontmatter.render(
    {
      ...(Option.isSome(rule.description) ? { description: rule.description.value } : {}),
      ...(rule.globs.length > 0 ? { globs: rule.globs.join(", ") } : {}),
      // `kind: "instructions"` rules land in the same rules dir, marked
      // alwaysApply — the closest Cursor gets to a memory file in-scope.
      ...(rule.alwaysApply || rule.kind === "instructions" ? { alwaysApply: true } : {}),
      ...extraFrontmatter(rule.metadata),
    },
    rule.body,
  );

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

/** Write `config` into the `.cursor` dir `dir`; returns per-file actions. */
export const write = (
  config: AgentConfig,
  dir: string,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const actions: ConfigWriteAction[] = [];

    actions.push(...(yield* writeDocs(fs, `${dir}/rules`, config.rules, ruleDoc)));
    actions.push(...(yield* writeSkillsDir(fs, `${dir}/skills`, config.skills)));
    actions.push(...(yield* writeDocs(fs, `${dir}/commands`, config.commands, commandDoc)));
    actions.push(...(yield* writeDocs(fs, `${dir}/agents`, config.agents, agentDoc)));

    if (config.hooks.length > 0) {
      const path = `${dir}/hooks.json`;
      const raw = (yield* readJsonIfExists(fs, path)) ?? {};
      const merged = mergeCursorHooks(raw, config.hooks);
      const text = `${JSON.stringify(merged, null, 2)}\n`;
      actions.push(yield* writeFileAction(fs, path, text));
    }

    const mcp = mergeMcpServers(yield* readJsonIfExists(fs, `${dir}/mcp.json`), config.mcpServers);
    if (mcp !== undefined) {
      actions.push(
        yield* writeFileAction(fs, `${dir}/mcp.json`, `${JSON.stringify(mcp, null, 2)}\n`),
      );
    }

    return actions;
  }).pipe(Effect.provide(fsLayer));
