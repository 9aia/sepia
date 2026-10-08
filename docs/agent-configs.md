# Agent config formats — Claude Code, Cursor, Cline, Devin

Reference for the on-disk **configuration layer** the four agents load before
a session starts — skills, rules, commands, hooks, subagents and MCP servers —
and what the config IR (`packages/sepia/src/AgentConfig.ts`) normalizes.
Verified against the real files on this machine (Oct 2026); the session-side
formats live in `docs/session-formats.md`.

> **Note (Rust rewrite).** The `packages/sepia/src/AgentConfig.ts` reference
> describes the deleted TypeScript tree. The config IR and on-disk formats it
> documents still hold; the Rust equivalents live under `crates/` (the IR in
> `crates/sepia-core`, per-agent config adapters in `crates/sepia-driver-*/src`,
> and the CLI verbs in `crates/sepia-cli/src/config_ops.rs`).

## The config IR

`AgentConfig` — `{skills[], rules[], commands[], hooks[], agents[],
mcpServers{}}`. Every item's `metadata` carries what the IR cannot express:
leftover frontmatter keys (passed back out verbatim on write) plus
`sourcePath` provenance.

| IR type         | Shape                                                                                        | Agents                                                                |
| --------------- | -------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `ConfigSkill`   | `{name, description?, body, files[]{path,content}, metadata}`                                | Claude, Cursor, Devin                                                 |
| `ConfigRule`    | `{name, description?, body, globs[], alwaysApply, kind, metadata}`                           | all four (`kind: "instructions"` = the single-file memory convention) |
| `ConfigCommand` | `{name, description?, body, argumentHint?, allowedTools[], model?, metadata}`                | all four                                                              |
| `ConfigHook`    | `{event, matcher?, type, command?, prompt?, timeoutSec?, failClosed?, loopLimit?, metadata}` | Claude, Cursor, Devin                                                 |
| `ConfigAgent`   | `{name, description?, body, tools[], model?, metadata}`                                      | Claude, Cursor                                                        |
| `mcpServers`    | `{name: {…}}` passthrough                                                                    | Claude, Cursor, Cline                                                 |

`event` is **canonical PascalCase** (`PreToolUse`, `UserPromptSubmit`,
`Stop`, …). `canonicalHookEvent`/`cursorHookEvent`/`claudeHookEvent`
translate to each store's wire spelling; cursor-only events
(`beforeShellExecution`, `afterFileEdit`, tab events, `postToolUseFailure`,
`afterAgentThought`) have no Claude/Devin counterpart — they keep their
canonical spelling and are skipped only where the target lacks the event
entirely (they still write, under the closest name).

## Claude Code — `~/.claude` (user), `<repo>/.claude` (project)

| Surface   | File(s)                                                                            | IR                                                                                |
| --------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Memory    | `CLAUDE.md` (in the dir, and the project-root one for `.claude` dirs)              | `ConfigRule{kind:"instructions", alwaysApply:true}` — `claude` / `claude-project` |
| Skills    | `skills/<name>/SKILL.md` + any sibling files                                       | `ConfigSkill` (+ `files[]`)                                                       |
| Commands  | `commands/<name>.md` — fm `description`, `argument-hint`, `allowed-tools`, `model` | `ConfigCommand`                                                                   |
| Subagents | `agents/<name>.md` — fm `name`, `description`, `tools`, `model`                    | `ConfigAgent`                                                                     |
| Hooks     | `settings.json` + `settings.local.json` `hooks` object                             | `ConfigHook` (`metadata.source` names the file)                                   |
| MCP       | `~/.claude.json` `mcpServers` (user dir); sibling `.mcp.json` (project dir)        | `mcpServers`                                                                      |

Hook wire shape (settings.json):

```jsonc
"hooks": { "PostToolUse": [ { "matcher": "Edit|Write",
  "hooks": [ {"type": "command", "command": "./fmt.sh", "timeout": 30} ] } ] }
```

Events: `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`,
`PostToolUse`, `Notification`, `Stop`, `SubagentStop`, `PreCompact`.

**Write mapping**: skills/commands/agents one-to-one; hooks merge into
`settings.json` under matcher groups, deduped; rules collapse into a managed
`<!-- sepia:rules -->…<!-- /sepia:rules -->` block inside `CLAUDE.md` (Claude
has no rules directory — glob scoping is rendered as a `> Applies to:` line);
`mcpServers` merge into `~/.claude.json` (user) or `.mcp.json` (project).

**Lossy**: rule glob scoping (inert inside CLAUDE.md); `settings.local.json`
vs `settings.json` provenance collapses on write; hook entries' extra fields
beyond type/command/prompt/timeout.

## Cursor — `~/.cursor` (user), `<repo>/.cursor` (project)

| Surface   | File(s)                                                                              | IR              |
| --------- | ------------------------------------------------------------------------------------ | --------------- |
| Rules     | `rules/*.{md,mdc}` — fm `description`, `globs` (comma string or list), `alwaysApply` | `ConfigRule`    |
| Commands  | `commands/<name>.md`                                                                 | `ConfigCommand` |
| Subagents | `agents/<name>.md` — fm `name`, `description` (+ `model`, `readonly`, …)             | `ConfigAgent`   |
| Skills    | `skills/<name>/SKILL.md` (`skills-cursor/` is Cursor's built-in tree — not scanned)  | `ConfigSkill`   |
| Hooks     | `hooks.json` `{version:1, hooks:{event:[entry]}}`                                    | `ConfigHook`    |
| MCP       | `mcp.json` `mcpServers`                                                              | `mcpServers`    |

Hook wire shape (flat per-event entries, camelCase events):

```jsonc
{
  "version": 1,
  "hooks": {
    "afterFileEdit": [
      {
        "command": ".cursor/hooks/fmt.sh",
        "matcher": "Write|TabWrite",
        "failClosed": true,
        "timeout": 30,
        "loop_limit": 3,
      },
    ],
  },
}
```

Entries carry `command`, `type` (`command`|`prompt`), `prompt`, `timeout`,
`matcher` (JS regex), `failClosed`, `loop_limit` — all mapped 1:1.
`type:"prompt"` hooks → `ConfigHook{type:"prompt", prompt}` (Claude has no
counterpart and drops to `command`-only on write there — recorded lossy).

Cursor-only surfaces not in the IR: `cli-config.json` (model, permissions,
display), `plans/`, `extensions/`, `projects/` (transcripts), Tab rules.

**Lossy**: `.mdc` files write back as `.md` (both are read); the user-level
`~/.cursor` dir has no project-root side-channel.

## Cline — workspace root (`dir` = where the session runs)

| Surface   | File(s)                                                                                       | IR                             |
| --------- | --------------------------------------------------------------------------------------------- | ------------------------------ |
| Rules     | `.clinerules` (one file → `clinerules`, `kind:"instructions"`) or `.clinerules/<name>.md` dir | `ConfigRule{alwaysApply:true}` |
| Workflows | `.clinerules/workflows/<name>.md` — `/name` prompt templates                                  | `ConfigCommand`                |
| MCP       | `cline_mcp_settings.json` `mcpServers`                                                        | `mcpServers`                   |

Not in the IR (agent-side, not portable config): `~/.cline/data/settings/`
(provider auth, `planActMode`), sessions/checkpoints under `~/.cline/data`.
No skills/subagents/hooks concept — writes report those items `skipped`.
Global (`~/.cline`) rules dirs were not observed on this machine; the
adapter is workspace-scoped.

**Lossy**: none beyond the skipped kinds — `.clinerules` file writes back
as the equivalent directory form.

## Devin — `~/.config/devin` (user), `<repo>/.devin` (project)

| Surface      | File(s)                                                                                                                                 | IR                                                            |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Skills       | `skills/<name>/SKILL.md` + siblings — fm `name`, `description`, `argument-hint`, `subagent`, `allowed-tools`, `permissions`, `triggers` | `ConfigSkill` (extras ride in `metadata`, preserved on write) |
| Rules        | `rules/<name>.md` — plain markdown, always applied                                                                                      | `ConfigRule{alwaysApply:true}`                                |
| Instructions | `AGENTS.md` inside the dir                                                                                                              | `ConfigRule{kind:"instructions"}`                             |
| Workflows    | `workflows/<name>.md` — fm `description`; triggered by `/name`                                                                          | `ConfigCommand`                                               |
| Hooks        | `hooks.v1.json` — **bare event map** in Claude's group schema; scripts under `hooks/` are referenced by `command` strings               | `ConfigHook`                                                  |

`config.json` (model/org prefs), `cli/`, extension state — agent-side, not
in the IR. No subagent or MCP file conventions — writes report `skipped`.

**Lossy**: hook _scripts_ (`hooks/*.py`/`*.sh`) are referenced by `command`
strings — installing hooks to another agent carries the string, not the
script; Devin's skill `permissions`/`triggers` frontmatter keys are inert
for other agents (they survive in `metadata` for a Devin write-back).

## `sepia config` verbs

```bash
sepia config list    --from <agent>          # inventory one store's config
sepia config export  [out.json] --from <a>   # → config IR JSON (stdout or file)
sepia config import  <cfg.json> --to <b>     # IR JSON → target's files
sepia config install --from <a> --to <b>     # read a → write b directly
sepia config diff    --from <a> --to <b>     # name-level delta + hook/MCP sets
```

`<agent>` ∈ `claude|cursor|cline|devin`. Dir flags pick the store — and
imply the agent when `--from`/`--to` is omitted (same convention as the
session verbs):

| Flag           | Default           | Meaning                                          |
| -------------- | ----------------- | ------------------------------------------------ |
| `--claude-dir` | `~/.claude`       | any `.claude` dir (user or `<repo>/.claude`)     |
| `--cursor-dir` | `~/.cursor`       | any `.cursor` dir                                |
| `--cline-dir`  | `$PWD`            | a Cline **workspace root** (holds `.clinerules`) |
| `--devin-dir`  | `~/.config/devin` | `~/.config/devin` or `<repo>/.devin`             |

Writes merge, never clobber: JSON files (settings/hooks/mcp) are read,
merged and rewritten with other keys preserved; markdown files are
written per-item (`wrote`/`updated`/`unchanged` actions are printed);
nothing is deleted.
