import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import * as Fs from "@effect/platform/FileSystem";
import { Effect, Layer, Option } from "effect";
import {
  AgentConfig,
  ConfigCommand,
  ConfigRule,
  isObject,
  mergeMcpServers,
  readFileIfExists,
  readJsonIfExists,
  readMarkdownDir,
  safeFileStem,
  writeFileAction,
  type ConfigWriteAction,
  type MarkdownDoc,
} from "sepia-core";
import { StorageError } from "sepia-core";
import { Frontmatter } from "sepia-core";

/**
 * Cline config store — `dir` is the workspace root (where a session runs),
 * not `~/.cline/data`: Cline's portable config lives next to the code.
 *
 * Read:
 * - `.clinerules` — a plain-text file → one `kind: "instructions"` rule.
 * - `.clinerules/` — a directory of `*.md` files → rules (Cline applies
 *   every rule file unconditionally, so `alwaysApply: true`). A nested
 *   `workflows/` dir holds slash-command markdown → `ConfigCommand`s.
 * - `cline_mcp_settings.json` `mcpServers`.
 *
 * Write: rules land as `.clinerules/<name>.md` files (the directory form —
 * always valid, unlike the single `.clinerules` file), commands as
 * `.clinerules/workflows/<name>.md`, MCP merged into
 * `cline_mcp_settings.json`. Cline has no skills/subagents/hooks concept —
 * those items are reported `skipped`.
 */

const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);

const docToCommand = (doc: MarkdownDoc): ConfigCommand => {
  const { description, ...rest } = doc.attributes;
  return ConfigCommand.make({
    name: doc.stem,
    description: Option.fromNullable(typeof description === "string" ? description : undefined),
    body: doc.body,
    metadata: { ...rest, sourcePath: doc.path },
  });
};

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

const program = (dir: string) =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;

    const rules: ConfigRule[] = [];
    const rulesPath = `${dir}/.clinerules`;
    const info = yield* fs.stat(rulesPath).pipe(Effect.option);
    if (Option.isSome(info) && info.value.type === "File") {
      const text = yield* readFileIfExists(fs, rulesPath);
      if (text !== undefined) {
        rules.push(
          ConfigRule.make({
            name: "clinerules",
            body: text.trimEnd(),
            alwaysApply: true,
            kind: "instructions",
            metadata: { sourcePath: rulesPath },
          }),
        );
      }
    } else if (Option.isSome(info) && info.value.type === "Directory") {
      rules.push(...(yield* readMarkdownDir(fs, rulesPath, [".md"])).map(docToRule));
    }

    const commands = (yield* readMarkdownDir(fs, `${rulesPath}/workflows`, [".md"])).map(
      docToCommand,
    );

    const mcpJson = yield* readJsonIfExists(fs, `${dir}/cline_mcp_settings.json`);
    const mcpServers =
      mcpJson !== undefined && isObject(mcpJson["mcpServers"])
        ? (mcpJson["mcpServers"] as Record<string, unknown>)
        : {};

    return AgentConfig.make({
      skills: [],
      rules,
      commands: [...commands],
      hooks: [],
      agents: [],
      mcpServers,
      metadata: { source: "cline", dir },
    });
  });

/** Read the Cline workspace config under `dir` into the config IR. */
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

const skipped = (kind: string, count: number): ConfigWriteAction | undefined =>
  count === 0
    ? undefined
    : {
        path: ".clinerules",
        action: "skipped" as const,
        detail: `cline has no ${kind} concept — ${count} item(s) not written`,
      };

/** Write `config` into the Cline workspace `dir`; returns per-file actions. */
export const write = (
  config: AgentConfig,
  dir: string,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const actions: ConfigWriteAction[] = [];

    actions.push(...(yield* writeDocs(fs, `${dir}/.clinerules`, config.rules, ruleDoc)));
    actions.push(
      ...(yield* writeDocs(fs, `${dir}/.clinerules/workflows`, config.commands, commandDoc)),
    );

    const mcp = mergeMcpServers(
      yield* readJsonIfExists(fs, `${dir}/cline_mcp_settings.json`),
      config.mcpServers,
    );
    if (mcp !== undefined) {
      actions.push(
        yield* writeFileAction(
          fs,
          `${dir}/cline_mcp_settings.json`,
          `${JSON.stringify(mcp, null, 2)}\n`,
        ),
      );
    }

    for (const action of [
      skipped("skills", config.skills.length),
      skipped("subagents", config.agents.length),
      skipped("hooks", config.hooks.length),
    ]) {
      if (action !== undefined) actions.push(action);
    }

    return actions;
  }).pipe(Effect.provide(fsLayer));
