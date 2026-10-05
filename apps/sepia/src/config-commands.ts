import { Args, Command, Options } from "@effect/cli";
import * as Fs from "@effect/platform/FileSystem";
import { Console, Effect, Option } from "effect";
import { homedir } from "node:os";
import {
  AgentConfig,
  ClaudeConfig,
  ClineConfig,
  configFromJson,
  configToJson,
  ConversionError,
  CursorConfig,
  DevinConfig,
  type ConfigWriteAction,
} from "sepia-core";

/**
 * The `sepia config` verb group — reads an agent's on-disk configuration
 * (skills, rules, commands, hooks, subagents, MCP servers) into the
 * config IR, exports/imports it as JSON, and installs it into another
 * agent's store. Pure file ops: no running node, no sessions touched.
 */

const AGENTS = ["claude", "cursor", "cline", "devin"] as const;
type AgentId = (typeof AGENTS)[number];

const defaultDir = (agent: AgentId): string => {
  switch (agent) {
    case "claude":
      return `${homedir()}/.claude`;
    case "cursor":
      return `${homedir()}/.cursor`;
    case "cline":
      return process.cwd();
    case "devin":
      return `${homedir()}/.config/devin`;
  }
};

const claudeDirOption = Options.text("claude-dir").pipe(
  Options.optional,
  Options.withDescription("Path to a .claude dir (default: ~/.claude)"),
);
const cursorDirOption = Options.text("cursor-dir").pipe(
  Options.optional,
  Options.withDescription("Path to a .cursor dir (default: ~/.cursor)"),
);
const clineDirOption = Options.text("cline-dir").pipe(
  Options.optional,
  Options.withDescription("Path to a Cline workspace root (default: cwd)"),
);
const devinDirOption = Options.text("devin-dir").pipe(
  Options.optional,
  Options.withDescription("Path to a Devin config dir (default: ~/.config/devin)"),
);

const fromOption = Options.choice("from", AGENTS).pipe(
  Options.optional,
  Options.withDescription("Agent to read the config from"),
);
const toOption = Options.choice("to", AGENTS).pipe(
  Options.optional,
  Options.withDescription("Agent to write the config into"),
);

/** The four `--*-dir` flags resolved into an agent → dir map. */
interface ConfigDirs {
  readonly claude?: string;
  readonly cursor?: string;
  readonly cline?: string;
  readonly devin?: string;
}

const dirOf = (agent: AgentId, dirs: ConfigDirs): string => dirs[agent] ?? defaultDir(agent);

/**
 * Resolve which agent a role names. An explicit `--from`/`--to` wins;
 * otherwise a single explicit `--*-dir` names its agent (the flag would be
 * noise otherwise, same convention as the session verbs); last resort is
 * `fallback` — and when there is none the flag was required anyway.
 */
const resolveAgent = (
  flag: Option.Option<AgentId>,
  dirs: ConfigDirs,
  claimed: AgentId | undefined,
  fallback: AgentId,
): AgentId =>
  Option.getOrElse(flag, () => {
    const named = (Object.keys(dirs) as ReadonlyArray<AgentId>).find(
      (a) => dirs[a] !== undefined && a !== claimed,
    );
    return named ?? fallback;
  });

const readAgent = (agent: AgentId, dirs: ConfigDirs): Effect.Effect<AgentConfig, unknown> => {
  const dir = dirOf(agent, dirs);
  switch (agent) {
    case "claude":
      return ClaudeConfig.read(dir);
    case "cursor":
      return CursorConfig.read(dir);
    case "cline":
      return ClineConfig.read(dir);
    case "devin":
      return DevinConfig.read(dir);
  }
};

const writeAgent = (
  agent: AgentId,
  dirs: ConfigDirs,
  config: AgentConfig,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, unknown> => {
  const dir = dirOf(agent, dirs);
  switch (agent) {
    case "claude":
      return ClaudeConfig.write(config, dir);
    case "cursor":
      return CursorConfig.write(config, dir);
    case "cline":
      return ClineConfig.write(config, dir);
    case "devin":
      return DevinConfig.write(config, dir);
  }
};

const printActions = (actions: ReadonlyArray<ConfigWriteAction>, target: AgentId) =>
  Effect.forEach(actions, (a) =>
    Console.log(`${a.action.padEnd(9)} ${a.path}${a.detail === undefined ? "" : ` — ${a.detail}`}`),
  ).pipe(
    Effect.andThen(
      Console.log(
        `${actions.filter((a) => a.action === "wrote" || a.action === "updated" || a.action === "merged").length} file(s) written into the ${target} config`,
      ),
    ),
  );

const summarize = (config: AgentConfig): ReadonlyArray<string> => {
  const lines: string[] = [];
  const section = (title: string, items: ReadonlyArray<{ readonly name: string }>) => {
    if (items.length === 0) return;
    lines.push(`${title} (${items.length}):`);
    for (const item of items) lines.push(`  ${item.name}`);
  };
  section("Rules", config.rules);
  section("Commands", config.commands);
  section("Skills", config.skills);
  section("Subagents", config.agents);
  if (config.hooks.length > 0) {
    lines.push(`Hooks (${config.hooks.length}):`);
    for (const hook of config.hooks) {
      const cmd = Option.isSome(hook.command)
        ? hook.command.value
        : Option.isSome(hook.prompt)
          ? "[prompt]"
          : "";
      lines.push(
        `  ${hook.event}${Option.isSome(hook.matcher) ? ` ~ ${hook.matcher.value}` : ""} → ${cmd}`,
      );
    }
  }
  const mcp = Object.keys(config.mcpServers);
  if (mcp.length > 0) lines.push(`MCP servers (${mcp.length}): ${mcp.join(", ")}`);
  return lines.length === 0 ? ["(empty config)"] : lines;
};

const dirsOf = (o: {
  readonly claudeDir: Option.Option<string>;
  readonly cursorDir: Option.Option<string>;
  readonly clineDir: Option.Option<string>;
  readonly devinDir: Option.Option<string>;
}): ConfigDirs => ({
  claude: Option.getOrUndefined(o.claudeDir),
  cursor: Option.getOrUndefined(o.cursorDir),
  cline: Option.getOrUndefined(o.clineDir),
  devin: Option.getOrUndefined(o.devinDir),
});

const listCommand = Command.make(
  "list",
  {
    from: fromOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    clineDir: clineDirOption,
    devinDir: devinDirOption,
  },
  ({ from, claudeDir, cursorDir, clineDir, devinDir }) => {
    const dirs = dirsOf({ claudeDir, cursorDir, clineDir, devinDir });
    const source = resolveAgent(from, dirs, undefined, "claude");
    return Effect.gen(function* () {
      const config = yield* readAgent(source, dirs);
      yield* Console.log(`# ${source} config — ${dirOf(source, dirs)}`);
      for (const line of summarize(config)) yield* Console.log(line);
    });
  },
).pipe(
  Command.withDescription(
    "List an agent's config items (rules, skills, commands, hooks, subagents, MCP)",
  ),
);

const exportCommand = Command.make(
  "export",
  {
    out: Args.text({ name: "out" }).pipe(
      Args.optional,
      Args.withDescription("Output file for the config JSON; stdout when omitted"),
    ),
    from: fromOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    clineDir: clineDirOption,
    devinDir: devinDirOption,
  },
  ({ out, from, claudeDir, cursorDir, clineDir, devinDir }) => {
    const dirs = dirsOf({ claudeDir, cursorDir, clineDir, devinDir });
    const source = resolveAgent(from, dirs, undefined, "claude");
    return Effect.gen(function* () {
      const config = yield* readAgent(source, dirs);
      const json = JSON.stringify(configToJson(config), null, 2);
      const outPath = Option.getOrUndefined(out);
      if (outPath === undefined || outPath === "-") {
        yield* Console.log(json);
        return;
      }
      const fs = yield* Fs.FileSystem;
      yield* fs.writeFileString(outPath, `${json}\n`);
      yield* Console.log(`Exported ${source} config to ${outPath}`);
    });
  },
).pipe(Command.withDescription("Export an agent's config as IR JSON (stdout or a file)"));

const importCommand = Command.make(
  "import",
  {
    path: Args.text({ name: "path" }).pipe(
      Args.withDescription("Config JSON file produced by `sepia config export`"),
    ),
    to: toOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    clineDir: clineDirOption,
    devinDir: devinDirOption,
  },
  ({ path, to, claudeDir, cursorDir, clineDir, devinDir }) => {
    const dirs = dirsOf({ claudeDir, cursorDir, clineDir, devinDir });
    const target = resolveAgent(to, dirs, undefined, "cursor");
    return Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const raw = yield* fs
        .readFileString(path)
        .pipe(
          Effect.mapError(
            (cause) => new ConversionError({ message: `Config source not found: ${path}`, cause }),
          ),
        );
      const config = yield* Effect.try({
        try: () => configFromJson(JSON.parse(raw)),
        catch: (cause) =>
          new ConversionError({
            message: `Config source is not a config IR JSON: ${path}`,
            cause,
          }),
      });
      const actions = yield* writeAgent(target, dirs, config);
      yield* printActions(actions, target);
    });
  },
).pipe(Command.withDescription("Install a config IR JSON file into an agent's config store"));

const installCommand = Command.make(
  "install",
  {
    from: fromOption,
    to: toOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    clineDir: clineDirOption,
    devinDir: devinDirOption,
  },
  ({ from, to, claudeDir, cursorDir, clineDir, devinDir }) => {
    const dirs = dirsOf({ claudeDir, cursorDir, clineDir, devinDir });
    const target = resolveAgent(to, dirs, undefined, "cursor");
    const source = resolveAgent(from, dirs, target, "claude");
    return Effect.gen(function* () {
      const config = yield* readAgent(source, dirs);
      const actions = yield* writeAgent(target, dirs, config);
      yield* Console.log(`Installed ${source} config into ${target} (${dirOf(target, dirs)})`);
      yield* printActions(actions, target);
    });
  },
).pipe(
  Command.withDescription("Copy one agent's config into another's store (IR → target's files)"),
);

/** Strip per-source provenance so a diff compares content, not origin. */
const norm = (item: unknown): string => {
  const clone = JSON.parse(JSON.stringify(item)) as Record<string, unknown>;
  delete clone["metadata"];
  return JSON.stringify(clone);
};

const diffKind = (
  label: string,
  a: ReadonlyArray<{ readonly name: string }>,
  b: ReadonlyArray<{ readonly name: string }>,
): ReadonlyArray<string> => {
  const lines: string[] = [];
  const bMap = new Map(b.map((item) => [item.name, item]));
  const aMap = new Map(a.map((item) => [item.name, item]));
  for (const item of a) {
    const other = bMap.get(item.name);
    if (other === undefined) lines.push(`  ${label} only in source: ${item.name}`);
    else if (norm(item) !== norm(other)) lines.push(`  ${label} changed: ${item.name}`);
  }
  for (const item of b) {
    if (!aMap.has(item.name)) lines.push(`  ${label} only in target: ${item.name}`);
  }
  return lines;
};

const diffCommand = Command.make(
  "diff",
  {
    from: fromOption,
    to: toOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    clineDir: clineDirOption,
    devinDir: devinDirOption,
  },
  ({ from, to, claudeDir, cursorDir, clineDir, devinDir }) => {
    const dirs = dirsOf({ claudeDir, cursorDir, clineDir, devinDir });
    const target = resolveAgent(to, dirs, undefined, "cursor");
    const source = resolveAgent(from, dirs, target, "claude");
    return Effect.gen(function* () {
      const a = yield* readAgent(source, dirs);
      const b = yield* readAgent(target, dirs);
      const lines = [
        ...diffKind("rule", a.rules, b.rules),
        ...diffKind("command", a.commands, b.commands),
        ...diffKind("skill", a.skills, b.skills),
        ...diffKind("subagent", a.agents, b.agents),
      ];
      const hookKey = (h: (typeof a.hooks)[number]) => `${h.event}:${JSON.stringify(h)}`;
      const aHooks = new Set(a.hooks.map((h) => hookKey(h)));
      const bHooks = new Set(b.hooks.map((h) => hookKey(h)));
      const onlyA = a.hooks.filter((h) => !bHooks.has(hookKey(h))).length;
      const onlyB = b.hooks.filter((h) => !aHooks.has(hookKey(h))).length;
      if (onlyA > 0) lines.push(`  hooks only in source: ${onlyA}`);
      if (onlyB > 0) lines.push(`  hooks only in target: ${onlyB}`);
      for (const name of Object.keys(a.mcpServers)) {
        if (b.mcpServers[name] === undefined) lines.push(`  mcp server only in source: ${name}`);
        else if (JSON.stringify(a.mcpServers[name]) !== JSON.stringify(b.mcpServers[name]))
          lines.push(`  mcp server changed: ${name}`);
      }
      for (const name of Object.keys(b.mcpServers)) {
        if (a.mcpServers[name] === undefined) lines.push(`  mcp server only in target: ${name}`);
      }
      yield* Console.log(`# ${source} → ${target}`);
      if (lines.length === 0) yield* Console.log("  configs equivalent");
      else for (const line of lines) yield* Console.log(line);
    });
  },
).pipe(Command.withDescription("Compare two agents' configs (name-level, plus hook/MCP deltas)"));

export const configGroup = Command.make("config").pipe(
  Command.withSubcommands([listCommand, exportCommand, importCommand, installCommand, diffCommand]),
  Command.withDescription(
    "Config ops — export, install and diff agent configs (skills, rules, commands, hooks) via the config IR",
  ),
);
