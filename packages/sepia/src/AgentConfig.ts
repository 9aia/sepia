import * as Fs from "@effect/platform/FileSystem";
import { Effect, Option, Schema } from "effect";
import { StorageError } from "./Domain.js";
import * as Frontmatter from "./Frontmatter.js";

/**
 * Agent-config IR — the portable layer over the four agents' on-disk
 * configuration: skills, rules, commands, hooks and subagent definitions
 * (docs/agent-configs.md maps every field to the per-agent files).
 *
 * Where the session IR normalizes transcripts, this normalizes the markdown
 * + JSON files an agent loads before a session starts:
 *
 * - `ConfigSkill` — a `SKILL.md` package (`name`/`description` frontmatter,
 *   markdown body, plus the sibling files the skill dir ships).
 * - `ConfigRule` — always-on or glob-scoped guidance (`.mdc`/`.md` rules,
 *   `.devin/rules/*.md`, `.clinerules`; `kind: "instructions"` marks the
 *   single-file memory convention — `CLAUDE.md`, `AGENTS.md`).
 * - `ConfigCommand` — a user-invocable prompt template (`/cmd`): Claude
 *   `commands/*.md`, Cursor `commands/*.md`, Devin `workflows/*.md`, Cline
 *   `.clinerules/workflows/*.md`.
 * - `ConfigHook` — an event-triggered shell/prompt check (Claude
 *   `settings.json` + Devin `hooks.v1.json` matcher groups, Cursor
 *   `hooks.json` flat entries). `event` is canonical PascalCase; adapters
 *   translate (see `hookEvents`).
 * - `ConfigAgent` — a named subagent with its own system prompt and tool
 *   allowlist (`.claude/agents/*.md`, `.cursor/agents/*.md`).
 * - `mcpServers` — the `mcpServers` object of the agent's MCP file,
 *   passed through untouched.
 *
 * Every item's `metadata` carries what the IR cannot express: leftover
 * frontmatter keys (Devin `permissions`/`triggers`, Cursor `readonly`) and
 * the `sourcePath` provenance readers record.
 */

/** A supporting file a skill ships next to its `SKILL.md` (path is relative). */
export const ConfigFile = Schema.Struct({
  path: Schema.String,
  content: Schema.String,
});
export type ConfigFile = Schema.Schema.Type<typeof ConfigFile>;

const optString = Schema.OptionFromSelf(Schema.String).pipe(
  Schema.optionalWith({ default: () => Option.none() }),
);
const optNumber = Schema.OptionFromSelf(Schema.Number).pipe(
  Schema.optionalWith({ default: () => Option.none() }),
);
const optBool = Schema.OptionFromSelf(Schema.Boolean).pipe(
  Schema.optionalWith({ default: () => Option.none() }),
);
const stringArray = Schema.Array(Schema.String).pipe(Schema.optionalWith({ default: () => [] }));

export class ConfigSkill extends Schema.Class<ConfigSkill>("ConfigSkill")({
  name: Schema.String,
  description: optString,
  body: Schema.String.pipe(Schema.optionalWith({ default: () => "" })),
  files: Schema.Array(ConfigFile).pipe(Schema.optionalWith({ default: () => [] })),
  metadata: Schema.Unknown,
}) {}

/** `kind: "instructions"` = the agent's single always-loaded memory file. */
export const RuleKind = Schema.Literal("rule", "instructions");
export type RuleKind = Schema.Schema.Type<typeof RuleKind>;

export class ConfigRule extends Schema.Class<ConfigRule>("ConfigRule")({
  name: Schema.String,
  description: optString,
  body: Schema.String.pipe(Schema.optionalWith({ default: () => "" })),
  /**
   * Glob patterns the rule applies to (Cursor `globs:`). Empty means
   * unscoped — the agent decides by `alwaysApply`/`description`.
   */
  globs: stringArray,
  /** Always injected into context (Cursor `alwaysApply`, CLAUDE.md, .clinerules). */
  alwaysApply: Schema.Boolean.pipe(Schema.optionalWith({ default: () => false })),
  kind: RuleKind.pipe(Schema.optionalWith({ default: () => "rule" as const })),
  metadata: Schema.Unknown,
}) {}

export class ConfigCommand extends Schema.Class<ConfigCommand>("ConfigCommand")({
  name: Schema.String,
  description: optString,
  /** The prompt template the slash command injects. */
  body: Schema.String.pipe(Schema.optionalWith({ default: () => "" })),
  /** Claude `argument-hint` — shown while typing `/name <args>`. */
  argumentHint: optString,
  allowedTools: stringArray,
  model: optString,
  metadata: Schema.Unknown,
}) {}

export class ConfigAgent extends Schema.Class<ConfigAgent>("ConfigAgent")({
  name: Schema.String,
  description: optString,
  /** The subagent's system prompt. */
  body: Schema.String.pipe(Schema.optionalWith({ default: () => "" })),
  tools: stringArray,
  model: optString,
  metadata: Schema.Unknown,
}) {}

export const HookKind = Schema.Literal("command", "prompt");
export type HookKind = Schema.Schema.Type<typeof HookKind>;

export class ConfigHook extends Schema.Class<ConfigHook>("ConfigHook")({
  /**
   * Canonical PascalCase event name (`PreToolUse`, `UserPromptSubmit`,
   * `Stop`, …) — `hookEvents.toCursor`/`toClaude` translate the wire names;
   * events with no counterpart keep their canonical spelling and the
   * original rides in `metadata.sourceEvent`.
   */
  event: Schema.String,
  /** Regex the event payload must match (tool name, shell command, …). */
  matcher: optString,
  type: HookKind.pipe(Schema.optionalWith({ default: () => "command" as const })),
  command: optString,
  /** Cursor `"type": "prompt"` hooks — the check is a prompt, not a shell line. */
  prompt: optString,
  timeoutSec: optNumber,
  /** Cursor-only: a crashing hook blocks the action instead of passing. */
  failClosed: optBool,
  /** Cursor `loop_limit` on `stop`/`subagentStop` follow-up loops. */
  loopLimit: optNumber,
  metadata: Schema.Unknown,
}) {}

export class AgentConfig extends Schema.Class<AgentConfig>("AgentConfig")({
  skills: Schema.Array(ConfigSkill).pipe(Schema.optionalWith({ default: () => [] })),
  rules: Schema.Array(ConfigRule).pipe(Schema.optionalWith({ default: () => [] })),
  commands: Schema.Array(ConfigCommand).pipe(Schema.optionalWith({ default: () => [] })),
  hooks: Schema.Array(ConfigHook).pipe(Schema.optionalWith({ default: () => [] })),
  agents: Schema.Array(ConfigAgent).pipe(Schema.optionalWith({ default: () => [] })),
  mcpServers: Schema.Record({ key: Schema.String, value: Schema.Unknown }).pipe(
    Schema.optionalWith({ default: () => ({}) }),
  ),
  metadata: Schema.Unknown,
}) {}

/**
 * Canonical hook events — the union of Claude's `settings.json` events and
 * Cursor's `hooks.json` ones, keyed canonical → per-agent wire name. Claude
 * and Devin (`hooks.v1.json`) share the PascalCase schema, so the canonical
 * name doubles as their wire name.
 */
const CURSOR_HOOK_EVENTS: Record<string, string> = {
  SessionStart: "sessionStart",
  SessionEnd: "sessionEnd",
  UserPromptSubmit: "beforeSubmitPrompt",
  PreToolUse: "preToolUse",
  PostToolUse: "postToolUse",
  PostToolUseFailure: "postToolUseFailure",
  SubagentStart: "subagentStart",
  SubagentStop: "subagentStop",
  Stop: "stop",
  PreCompact: "preCompact",
  Notification: "afterAgentResponse",
  BeforeShellExecution: "beforeShellExecution",
  AfterShellExecution: "afterShellExecution",
  BeforeMCPExecution: "beforeMCPExecution",
  AfterMCPExecution: "afterMCPExecution",
  BeforeReadFile: "beforeReadFile",
  AfterFileEdit: "afterFileEdit",
  BeforeTabFileRead: "beforeTabFileRead",
  AfterTabFileEdit: "afterTabFileEdit",
  AfterAgentThought: "afterAgentThought",
};

const capitalize = (s: string): string => (s === "" ? s : s[0].toUpperCase() + s.slice(1));
const decapitalize = (s: string): string => (s === "" ? s : s[0].toLowerCase() + s.slice(1));

/** Cursor wire name → canonical (`beforeSubmitPrompt` → `UserPromptSubmit`). */
export const canonicalHookEvent = (wire: string): string => {
  for (const [canonical, cursor] of Object.entries(CURSOR_HOOK_EVENTS)) {
    if (cursor === wire) return canonical;
  }
  return wire.charAt(0) === wire.charAt(0).toLowerCase() ? capitalize(wire) : wire;
};

/** Canonical → Cursor wire name; unknown events fall back to camelCase. */
export const cursorHookEvent = (canonical: string): string =>
  CURSOR_HOOK_EVENTS[canonical] ?? decapitalize(canonical);

/** Canonical → Claude/Devin wire name (the schema is already PascalCase). */
export const claudeHookEvent = (canonical: string): string =>
  canonical.charAt(0) === canonical.charAt(0).toLowerCase() ? capitalize(canonical) : canonical;

// ---------------------------------------------------------------------------
// Wire JSON — `sepia config export` output / `config import` input.
// ---------------------------------------------------------------------------

// Wire item shapes: same fields as the classes, but `Option` rides as
// `field?: value` (`OptionFromUndefinedOr`) so the payload is plain JSON —
// the same convention `SessionJson` uses.
const wireOption = Schema.OptionFromUndefinedOr;

const SkillJson = Schema.Struct({
  name: Schema.String,
  description: wireOption(Schema.String),
  body: Schema.optionalWith(Schema.String, { default: () => "" }),
  files: Schema.optionalWith(Schema.Array(ConfigFile), { default: () => [] }),
  metadata: Schema.Unknown,
});

const RuleJson = Schema.Struct({
  name: Schema.String,
  description: wireOption(Schema.String),
  body: Schema.optionalWith(Schema.String, { default: () => "" }),
  globs: Schema.optionalWith(Schema.Array(Schema.String), { default: () => [] }),
  alwaysApply: Schema.optionalWith(Schema.Boolean, { default: () => false }),
  kind: Schema.optionalWith(RuleKind, { default: () => "rule" as const }),
  metadata: Schema.Unknown,
});

const CommandJson = Schema.Struct({
  name: Schema.String,
  description: wireOption(Schema.String),
  body: Schema.optionalWith(Schema.String, { default: () => "" }),
  argumentHint: wireOption(Schema.String),
  allowedTools: Schema.optionalWith(Schema.Array(Schema.String), { default: () => [] }),
  model: wireOption(Schema.String),
  metadata: Schema.Unknown,
});

const HookJson = Schema.Struct({
  event: Schema.String,
  matcher: wireOption(Schema.String),
  type: Schema.optionalWith(HookKind, { default: () => "command" as const }),
  command: wireOption(Schema.String),
  prompt: wireOption(Schema.String),
  timeoutSec: wireOption(Schema.Number),
  failClosed: wireOption(Schema.Boolean),
  loopLimit: wireOption(Schema.Number),
  metadata: Schema.Unknown,
});

const AgentJson = Schema.Struct({
  name: Schema.String,
  description: wireOption(Schema.String),
  body: Schema.optionalWith(Schema.String, { default: () => "" }),
  tools: Schema.optionalWith(Schema.Array(Schema.String), { default: () => [] }),
  model: wireOption(Schema.String),
  metadata: Schema.Unknown,
});

const AgentConfigJson = Schema.Struct({
  version: Schema.Literal(1).pipe(Schema.optionalWith({ default: () => 1 as const })),
  skills: Schema.Array(SkillJson).pipe(Schema.optionalWith({ default: () => [] })),
  rules: Schema.Array(RuleJson).pipe(Schema.optionalWith({ default: () => [] })),
  commands: Schema.Array(CommandJson).pipe(Schema.optionalWith({ default: () => [] })),
  hooks: Schema.Array(HookJson).pipe(Schema.optionalWith({ default: () => [] })),
  agents: Schema.Array(AgentJson).pipe(Schema.optionalWith({ default: () => [] })),
  mcpServers: Schema.Record({ key: Schema.String, value: Schema.Unknown }).pipe(
    Schema.optionalWith({ default: () => ({}) }),
  ),
});

/** Serialize an `AgentConfig` to the `config export` JSON payload. */
export const configToJson = (config: AgentConfig): Record<string, unknown> => ({
  version: 1,
  skills: config.skills.map((s) => Schema.encodeSync(SkillJson)(s)),
  rules: config.rules.map((r) => Schema.encodeSync(RuleJson)(r)),
  commands: config.commands.map((c) => Schema.encodeSync(CommandJson)(c)),
  hooks: config.hooks.map((h) => Schema.encodeSync(HookJson)(h)),
  agents: config.agents.map((a) => Schema.encodeSync(AgentJson)(a)),
  mcpServers: config.mcpServers,
});

/** Decode a config JSON payload; throws `ParseError` on a non-IR shape. */
export const configFromJson = (input: unknown): AgentConfig => {
  const decoded = Schema.decodeUnknownSync(AgentConfigJson)(input);
  return AgentConfig.make({
    skills: decoded.skills.map((s) => ConfigSkill.make(s)),
    rules: decoded.rules.map((r) => ConfigRule.make(r)),
    commands: decoded.commands.map((c) => ConfigCommand.make(c)),
    hooks: decoded.hooks.map((h) => ConfigHook.make(h)),
    agents: decoded.agents.map((a) => ConfigAgent.make(a)),
    mcpServers: decoded.mcpServers,
    metadata: null,
  });
};

// ---------------------------------------------------------------------------
// Shared adapter plumbing — write reports, name/path safety, fs helpers.
// ---------------------------------------------------------------------------

/** What a writer did to one file — the CLI prints these. */
export interface ConfigWriteAction {
  readonly path: string;
  readonly action: "wrote" | "updated" | "unchanged" | "merged" | "skipped";
  readonly detail?: string;
}

/**
 * Item names become path segments (`skills/<name>/`, `rules/<name>.md`), so
 * they must be a single safe stem — no separators, NUL, dot-dirs, leading
 * dots or whitespace. Returns the sanitized stem or null when nothing
 * usable remains.
 */
export const safeFileStem = (name: string): string | null => {
  const cleaned = name
    .trim()
    .replace(/[/\\\0]/g, "-")
    .replace(/^\.+/, "")
    .replace(/\s+/g, "-")
    .replace(/\.(md|mdc)$/i, "");
  return cleaned === "" || cleaned === "." || cleaned === ".." ? null : cleaned;
};

const fsError = (prefix: string) => (cause: unknown) =>
  new StorageError({
    message: `${prefix}: ${cause instanceof Error ? cause.message : String(cause)}`,
  });

/** A missing path — absent file or a segment that isn't a directory. */
const isNotFound = (e: StorageError): boolean =>
  e.message.includes("ENOENT") ||
  e.message.includes("no such file") ||
  e.message.includes("ENOTDIR") ||
  e.message.includes("not a directory");

/** Read a file as UTF-8, or undefined when absent; other errors propagate. */
export const readFileIfExists = (
  fs: Fs.FileSystem,
  path: string,
): Effect.Effect<string | undefined, StorageError> =>
  fs.readFileString(path).pipe(
    Effect.mapError(fsError(`Failed to read ${path}`)),
    Effect.catchTag("StorageError", (e) =>
      isNotFound(e) ? Effect.succeed(undefined) : Effect.fail(e),
    ),
  );

/** Read + JSON.parse a file, or undefined when absent/unparseable. */
export const readJsonIfExists = (
  fs: Fs.FileSystem,
  path: string,
): Effect.Effect<Record<string, unknown> | undefined, StorageError> =>
  Effect.map(readFileIfExists(fs, path), (text) => {
    if (text === undefined) return undefined;
    try {
      const parsed = JSON.parse(text) as unknown;
      return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
        ? (parsed as Record<string, unknown>)
        : undefined;
    } catch {
      return undefined;
    }
  });

/** List directory entries, or [] when the directory is absent. */
export const listDirIfExists = (
  fs: Fs.FileSystem,
  path: string,
): Effect.Effect<ReadonlyArray<string>, StorageError> =>
  fs.readDirectory(path).pipe(
    Effect.mapError(fsError(`Failed to list ${path}`)),
    Effect.catchTag("StorageError", (e) =>
      isNotFound(e) ? Effect.succeed([] as ReadonlyArray<string>) : Effect.fail(e),
    ),
  );

/**
 * Write `content` to `path`, creating parents — returns the action the
 * write resolved to (`unchanged` when the file already held it).
 */
export const writeFileAction = (
  fs: Fs.FileSystem,
  path: string,
  content: string,
): Effect.Effect<ConfigWriteAction, StorageError> =>
  Effect.gen(function* () {
    const existing = yield* readFileIfExists(fs, path);
    if (existing === content) return { path, action: "unchanged" as const };
    const dir = path.slice(0, path.lastIndexOf("/"));
    if (dir !== "") {
      yield* fs
        .makeDirectory(dir, { recursive: true })
        .pipe(Effect.mapError(fsError(`Failed to create ${dir}`)));
    }
    yield* fs
      .writeFileString(path, content)
      .pipe(Effect.mapError(fsError(`Failed to write ${path}`)));
    return { path, action: existing === undefined ? ("wrote" as const) : ("updated" as const) };
  });

// ---------------------------------------------------------------------------
// Shared wire shapes — Claude/Devin hook schema, the memory-file block, and
// frontmatter field coercion adapters all need.
// ---------------------------------------------------------------------------

export const isObject = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/** `globs`/`tools`/`allowed-tools` accept a comma string or a list. */
export const stringList = (value: unknown): ReadonlyArray<string> => {
  if (typeof value === "string") {
    return value
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s !== "");
  }
  if (Array.isArray(value)) {
    return value.filter((v): v is string => typeof v === "string" && v !== "");
  }
  return [];
};

const strOrUndef = (value: unknown): string | undefined =>
  typeof value === "string" && value !== "" ? value : undefined;

const numOrUndef = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

/**
 * Flatten the Claude/Devin hook schema into `ConfigHook`s. Two containers
 * share the group shape `{Event: [{matcher?, hooks: [{type, command?,
 * prompt?, timeout?}]}]}`: Claude `settings.json`/`settings.local.json`
 * nest it under a `hooks` key while Devin `hooks.v1.json` is the bare
 * event map — this accepts either (a `hooks` key wins when it is an
 * object). `source` tags each hook's metadata with the file it came from.
 */
export const hooksFromClaudeJson = (
  settings: Record<string, unknown>,
  source: string,
): ReadonlyArray<ConfigHook> => {
  const nested = settings["hooks"];
  const hooks = isObject(nested) ? nested : settings;
  const out: ConfigHook[] = [];
  for (const [event, groups] of Object.entries(hooks)) {
    if (!Array.isArray(groups)) continue;
    for (const group of groups) {
      if (!isObject(group)) continue;
      const matcher = strOrUndef(group["matcher"]);
      const entries = group["hooks"];
      if (!Array.isArray(entries)) continue;
      for (const entry of entries) {
        if (!isObject(entry)) continue;
        out.push(
          ConfigHook.make({
            event: claudeHookEvent(event),
            matcher: Option.fromNullable(matcher),
            type: entry["type"] === "prompt" ? "prompt" : "command",
            command: Option.fromNullable(strOrUndef(entry["command"])),
            prompt: Option.fromNullable(strOrUndef(entry["prompt"])),
            timeoutSec: Option.fromNullable(numOrUndef(entry["timeout"])),
            metadata: { source },
          }),
        );
      }
    }
  }
  return out;
};

const claudeHookEntry = (hook: ConfigHook): Record<string, unknown> => ({
  type: hook.type,
  ...(Option.isSome(hook.command) ? { command: hook.command.value } : {}),
  ...(Option.isSome(hook.prompt) ? { prompt: hook.prompt.value } : {}),
  ...(Option.isSome(hook.timeoutSec) ? { timeout: hook.timeoutSec.value } : {}),
});

/**
 * Merge `hooks` into a bare event map — Devin's `hooks.v1.json` shape —
 * preserving existing events. New entries append to a matcher group that
 * shares the hook's matcher; exact duplicates (same type+command+prompt+
 * timeout inside the same matcher group) are not re-added.
 */
export const mergeHookEvents = (
  existing: Record<string, unknown>,
  hooks: ReadonlyArray<ConfigHook>,
): Record<string, unknown> => {
  const merged: Record<string, unknown> = { ...existing };
  for (const hook of hooks) {
    const event = claudeHookEvent(hook.event);
    const matcher = Option.getOrElse(hook.matcher, () => "");
    const groups = Array.isArray(merged[event]) ? (merged[event] as ReadonlyArray<unknown>) : [];
    const entry = claudeHookEntry(hook);
    const entryKey = JSON.stringify(entry);
    const group = groups.find(
      (g) => isObject(g) && strOrUndef(g["matcher"]) === (matcher === "" ? undefined : matcher),
    );
    if (group !== undefined && isObject(group)) {
      const entries = Array.isArray(group["hooks"]) ? (group["hooks"] as unknown[]) : [];
      if (!entries.some((e) => JSON.stringify(e) === entryKey)) {
        group["hooks"] = [...entries, entry];
      }
      merged[event] = groups;
    } else {
      merged[event] = [...groups, { ...(matcher === "" ? {} : { matcher }), hooks: [entry] }];
    }
  }
  return merged;
};

/**
 * Merge `hooks` into a Claude settings object's `hooks` block
 * (`settings.json`), preserving every other key — the `mergeHookEvents`
 * wrapper for the nested container.
 */
export const mergeClaudeHooks = (
  settings: Record<string, unknown>,
  hooks: ReadonlyArray<ConfigHook>,
): Record<string, unknown> => {
  const existing = isObject(settings["hooks"])
    ? (settings["hooks"] as Record<string, unknown>)
    : {};
  return { ...settings, hooks: mergeHookEvents(existing, hooks) };
};

/** Marker pair wrapping the rules block sepia manages inside a memory file. */
export const RULES_BLOCK_BEGIN = "<!-- sepia:rules -->";
export const RULES_BLOCK_END = "<!-- /sepia:rules -->";

const renderRule = (rule: ConfigRule): string => {
  const scope = rule.globs.length > 0 ? `\n> Applies to: ${rule.globs.join(", ")}\n` : "";
  const desc = Option.isSome(rule.description) ? `\n> ${rule.description.value}\n` : "";
  return `## ${rule.name}\n${desc}${scope}\n${rule.body.trim()}\n`;
};

/**
 * Merge `rules` into a memory file (`CLAUDE.md`, `AGENTS.md`): everything
 * the IR carries lives between the `sepia:rules` markers, replacing a
 * previous block and preserving whatever the file held outside it. Returns
 * undefined when there is nothing to write — no rules and no stale block.
 */
export const rulesToMemoryFile = (
  existing: string | undefined,
  rules: ReadonlyArray<ConfigRule>,
): string | undefined => {
  const text = existing ?? "";
  const begin = text.indexOf(RULES_BLOCK_BEGIN);
  const end = text.indexOf(RULES_BLOCK_END);
  const hadBlock = begin !== -1 && end > begin;
  if (rules.length === 0 && !hadBlock) return undefined;
  const block = `${RULES_BLOCK_BEGIN}\n\n${rules.map(renderRule).join("\n")}\n${RULES_BLOCK_END}`;
  if (!hadBlock) {
    const base = text.trimEnd();
    return base === "" ? `${block}\n` : `${base}\n\n${block}\n`;
  }
  const after = end + RULES_BLOCK_END.length;
  return `${text.slice(0, begin)}${block}${text.slice(after)}`;
};

/**
 * Merge `mcpServers` into an MCP JSON object (`{mcpServers: {...}}`),
 * preserving other top-level keys and existing servers.
 */
export const mergeMcpServers = (
  existing: Record<string, unknown> | undefined,
  servers: Record<string, unknown>,
): Record<string, unknown> | undefined => {
  const names = Object.keys(servers);
  if (names.length === 0) return undefined;
  const base = existing ?? {};
  const current = isObject(base["mcpServers"])
    ? (base["mcpServers"] as Record<string, unknown>)
    : {};
  return { ...base, mcpServers: { ...current, ...servers } };
};

// ---------------------------------------------------------------------------
// Shared walkers — every adapter scans `*.md` dirs and `<name>/SKILL.md`
// packages with the same rules.
// ---------------------------------------------------------------------------

/** A parsed `*.md`/`*.mdc` doc: file stem, frontmatter attributes, body. */
export interface MarkdownDoc {
  readonly stem: string;
  readonly fileName: string;
  readonly path: string;
  readonly attributes: Record<string, unknown>;
  readonly body: string;
}

/** Read every top-level file in `dir` whose extension is in `exts`, sorted. */
export const readMarkdownDir = (
  fs: Fs.FileSystem,
  dir: string,
  exts: ReadonlyArray<string>,
): Effect.Effect<ReadonlyArray<MarkdownDoc>, StorageError> =>
  Effect.gen(function* () {
    const names = yield* listDirIfExists(fs, dir);
    const docs: MarkdownDoc[] = [];
    for (const name of [...names].sort()) {
      const ext = exts.find((e) => name.toLowerCase().endsWith(e));
      if (ext === undefined) continue;
      const path = `${dir}/${name}`;
      const text = yield* readFileIfExists(fs, path);
      if (text === undefined) continue;
      const { attributes, body } = Frontmatter.parse(text);
      docs.push({
        stem: name.slice(0, name.length - ext.length),
        fileName: name,
        path,
        attributes,
        body,
      });
    }
    return docs;
  });

const walkFiles = (
  fs: Fs.FileSystem,
  dir: string,
  prefix: string,
): Effect.Effect<ReadonlyArray<string>, StorageError> =>
  Effect.gen(function* () {
    const names = yield* listDirIfExists(fs, dir);
    const out: string[] = [];
    for (const name of names) {
      const path = `${dir}/${name}`;
      const info = yield* fs.stat(path).pipe(Effect.mapError(fsError(`Failed to stat ${path}`)));
      if (info.type === "Directory") {
        out.push(...(yield* walkFiles(fs, path, `${prefix}${name}/`)));
      } else if (info.type === "File") {
        out.push(`${prefix}${name}`);
      }
    }
    return out;
  });

/**
 * Read a skills root (`<dir>/<name>/SKILL.md` plus sibling files). A dir
 * without a `SKILL.md` is skipped; frontmatter `name` wins over the dir
 * name; leftover attributes land in `metadata` for a lossless write-back.
 */
export const readSkillsDir = (
  fs: Fs.FileSystem,
  skillsDir: string,
): Effect.Effect<ReadonlyArray<ConfigSkill>, StorageError> =>
  Effect.gen(function* () {
    const names = yield* listDirIfExists(fs, skillsDir);
    const skills: ConfigSkill[] = [];
    for (const name of [...names].sort()) {
      const dir = `${skillsDir}/${name}`;
      const skillPath = `${dir}/SKILL.md`;
      const text = yield* readFileIfExists(fs, skillPath);
      if (text === undefined) continue;
      const { attributes, body } = Frontmatter.parse(text);
      const { name: attrName, description, ...rest } = attributes;
      const relFiles = (yield* walkFiles(fs, dir, "")).filter(
        (p) => p.toUpperCase() !== "SKILL.MD",
      );
      const files: ConfigFile[] = [];
      for (const rel of relFiles.sort()) {
        const content = yield* readFileIfExists(fs, `${dir}/${rel}`);
        if (content !== undefined) files.push({ path: rel, content });
      }
      skills.push(
        ConfigSkill.make({
          name: typeof attrName === "string" && attrName !== "" ? attrName : name,
          description: Option.fromNullable(
            typeof description === "string" && description !== "" ? description : undefined,
          ),
          body,
          files,
          metadata: { ...rest, sourcePath: skillPath },
        }),
      );
    }
    return skills;
  });

/** Frontmatter attributes a skill write emits: name/description + leftovers. */
export const skillAttributes = (skill: ConfigSkill): Record<string, unknown> => {
  const meta = isObject(skill.metadata) ? skill.metadata : {};
  const { sourcePath: _omitPath, source: _omitSource, ...rest } = meta;
  return {
    name: skill.name,
    ...(Option.isSome(skill.description) ? { description: skill.description.value } : {}),
    ...rest,
  };
};

/** Write `skills` under `skillsDir` — `<name>/SKILL.md` plus sibling files. */
export const writeSkillsDir = (
  fs: Fs.FileSystem,
  skillsDir: string,
  skills: ReadonlyArray<ConfigSkill>,
): Effect.Effect<ReadonlyArray<ConfigWriteAction>, StorageError> =>
  Effect.gen(function* () {
    const actions: ConfigWriteAction[] = [];
    for (const skill of skills) {
      const stem = safeFileStem(skill.name);
      if (stem === null) {
        actions.push({
          path: skillsDir,
          action: "skipped" as const,
          detail: `skill ${JSON.stringify(skill.name)} has no usable file stem`,
        });
        continue;
      }
      const dir = `${skillsDir}/${stem}`;
      actions.push(
        yield* writeFileAction(
          fs,
          `${dir}/SKILL.md`,
          Frontmatter.render(skillAttributes(skill), skill.body),
        ),
      );
      for (const file of skill.files) {
        // Preserve subpaths; only refuse genuinely escaping names.
        if (file.path.includes("..") || file.path.startsWith("/")) {
          actions.push({
            path: `${dir}/${file.path}`,
            action: "skipped" as const,
            detail: "skill file path escapes the skill dir",
          });
          continue;
        }
        actions.push(yield* writeFileAction(fs, `${dir}/${file.path}`, file.content));
      }
    }
    return actions;
  });
