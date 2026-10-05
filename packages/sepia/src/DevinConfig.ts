import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import { Effect, Layer, Option } from "effect";
import {
  AgentConfig,
  ConfigCommand,
  ConfigRule,
  hooksFromClaudeJson,
  mergeHookEvents,
  readFileIfExists,
  readJsonIfExists,
  readMarkdownDir,
  readSkillsDir,
  rulesToMemoryFile,
  safeFileStem,
  writeFileAction,
  writeSkillsDir,
  type ConfigWriteAction,
  type MarkdownDoc,
} from "./AgentConfig.js";
import { StorageError } from "./Domain.js";
import * as Frontmatter from "./Frontmatter.js";

/**
 * Devin config store — `dir` is `~/.config/devin` (user scope) or a repo's
 * `.devin/` directory (project scope).
 *
 * Read:
 * - `skills/<name>/SKILL.md` + sibling files — Devin skills carry rich
 *   frontmatter (`argument-hint`, `subagent`, `allowed-tools`,
 *   `permissions`, `triggers`); leftovers ride in `metadata`.
 * - `rules/<name>.md` — plain markdown rules Devin always applies.
 * - `workflows/<name>.md` — `/name` prompt templates → `ConfigCommand`s.
 * - `AGENTS.md` inside the dir → a `kind: "instructions"` rule.
 * - `hooks.v1.json` — the Claude settings-schema hooks object
 *   (`{Event: [{matcher, hooks: [{type, command, timeout}]}]}`); hook
 *   *scripts* under `hooks/` are referenced by `command` strings and stay
 *   the user's responsibility.
 * - `config.json` (model/org preferences) is not prompt config — skipped.
 *
 * Write: skills/rules/commands symmetric; `kind: "instructions"` rules
 * merge into `AGENTS.md` under a managed `sepia:rules` block; hooks merge
 * into `hooks.v1.json`; subagent and MCP items report `skipped`.
 */

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const docToRule = (doc: MarkdownDoc): ConfigRule => {
  const { description, ...rest } = doc.attributes;
  return ConfigRule.make({
    name: doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    alwaysApply: true,
    kind: "rule",
    metadata: { ...rest, sourcePath: doc.path },
  });
};

const docToCommand = (doc: MarkdownDoc): ConfigCommand => {
  const { description, ...rest } = doc.attributes;
  return ConfigCommand.make({
    name: doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    metadata: { ...rest, sourcePath: doc.path },
  });
};

const program = (dir: string) =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;

    const skills = yield* readSkillsDir(fs, `${dir}/skills`);
    const rules: ConfigRule[] = (yield* readMarkdownDir(fs, `${dir}/rules`, [".md"])).map(
      docToRule,
    );
    const commands = (yield* readMarkdownDir(fs, `${dir}/workflows`, [".md"])).map(docToCommand);

    const agentsMd = yield* readFileIfExists(fs, `${dir}/AGENTS.md`);
    if (agentsMd !== undefined) {
      rules.push(
        ConfigRule.make({
          name: "agents",
          body: agentsMd.trimEnd(),
          alwaysApply: true,
          kind: "instructions",
          metadata: { sourcePath: `${dir}/AGENTS.md` },
        }),
      );
    }

    const hooksJson = yield* readJsonIfExists(fs, `${dir}/hooks.v1.json`);
    const hooks = hooksJson === undefined ? [] : hooksFromClaudeJson(hooksJson, "hooks.v1.json");

    return AgentConfig.make({
      skills: [...skills],
      rules,
      commands: [...commands],
      hooks: [...hooks],
      agents: [],
      mcpServers: {},
      metadata: { source: "devin", dir },
    });
  });

/** Read the Devin config under `dir` into the config IR. */
export const read = (dir: string): Effect.Effect<AgentConfig, StorageError> =>
  program(dir).pipe(Effect.provide(fsLayer));

const ruleDoc = (rule: ConfigRule): string =>
  Frontmatter.render(
    Option.isSome(rule.description) ? { description: rule.description.value } : {},
    rule.body,
  );

const commandDoc = (command: ConfigCommand): string =>
  Frontmatter.render(
    Option.isSome(command.description) ? { description: command.description.value } : {},
    command.body,
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

/** Write `config` into the Devin config `dir`; returns per-file actions. */
export const write = (
  config: AgentConfig,
  dir: string,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const actions: ConfigWriteAction[] = [];

    actions.push(...(yield* writeSkillsDir(fs, `${dir}/skills`, config.skills)));

    const fileRules = config.rules.filter((r) => r.kind !== "instructions");
    const memoryRules = config.rules.filter((r) => r.kind === "instructions");
    actions.push(...(yield* writeDocs(fs, `${dir}/rules`, fileRules, ruleDoc)));
    if (memoryRules.length > 0) {
      const path = `${dir}/AGENTS.md`;
      const existing = yield* readFileIfExists(fs, path);
      const merged = rulesToMemoryFile(existing, memoryRules);
      if (merged !== undefined) actions.push(yield* writeFileAction(fs, path, merged));
    }

    actions.push(...(yield* writeDocs(fs, `${dir}/workflows`, config.commands, commandDoc)));

    if (config.hooks.length > 0) {
      const path = `${dir}/hooks.v1.json`;
      const raw = (yield* readJsonIfExists(fs, path)) ?? {};
      const merged = mergeHookEvents(raw, config.hooks);
      actions.push(yield* writeFileAction(fs, path, `${JSON.stringify(merged, null, 2)}\n`));
    }

    if (config.agents.length > 0) {
      actions.push({
        path: dir,
        action: "skipped" as const,
        detail: `devin has no subagent files — ${config.agents.length} item(s) not written`,
      });
    }
    if (Object.keys(config.mcpServers).length > 0) {
      actions.push({
        path: dir,
        action: "skipped" as const,
        detail: "devin has no MCP settings file — mcpServers not written",
      });
    }

    return actions;
  }).pipe(Effect.provide(fsLayer));
