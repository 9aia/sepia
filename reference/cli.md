# Command-Line Help for `sepia`

This document contains the help content for the `sepia` command-line program.

Inspect and manage coding-agent sessions agnostically — local stores, agent configs, drivers, and the session plane itself

**Command Overview:**

* [`sepia`↴](#sepia)
* [`sepia driver`↴](#sepia-driver)
* [`sepia driver list`↴](#sepia-driver-list)
* [`sepia config`↴](#sepia-config)
* [`sepia config list`↴](#sepia-config-list)
* [`sepia config export`↴](#sepia-config-export)
* [`sepia config import`↴](#sepia-config-import)
* [`sepia config install`↴](#sepia-config-install)
* [`sepia config diff`↴](#sepia-config-diff)
* [`sepia store`↴](#sepia-store)
* [`sepia store list`↴](#sepia-store-list)
* [`sepia store export`↴](#sepia-store-export)
* [`sepia store import`↴](#sepia-store-import)
* [`sepia store install`↴](#sepia-store-install)
* [`sepia store delete`↴](#sepia-store-delete)
* [`sepia list`↴](#sepia-list)
* [`sepia export`↴](#sepia-export)
* [`sepia import`↴](#sepia-import)
* [`sepia install`↴](#sepia-install)
* [`sepia delete`↴](#sepia-delete)
* [`sepia serve`↴](#sepia-serve)
* [`sepia version`↴](#sepia-version)

## `sepia`

Inspect and manage coding-agent sessions agnostically — local stores, agent configs, drivers, and the session plane itself

**Usage:** `sepia <COMMAND>`

###### **Commands:**

* `driver` — Local driver discovery — the sepia-driver-* binaries the session plane loads (SEPIA_DRIVER_DIR → platform data dir → PATH). `sepia driver` alone lists them
* `config` — Agent config IR ops — list/export/import/install/diff rules, skills, commands, hooks, subagents and MCP servers
* `store` — Store ops — read and write local agent stores directly
* `list` — List sessions in a store
* `export` — Export a session from a store — the session IR JSON (default) or Cline session files
* `import` — Import a session (Cline dir, Claude .jsonl, or session JSON) into a store
* `install` — Copy a session into a store ready to resume — cline (default), claude, cursor or devin
* `delete` — Delete a session from a store
* `serve` — Serve the local session plane — ndjson JSON-RPC over stdio (spawned and driven by orchestrators like peregrin)
* `version` — Print the sepia version stamp

###### **Options:**

* `-h, --help` — Print help
* `-V, --version` — Print version

## `sepia driver`

Local driver discovery — the sepia-driver-* binaries the session plane loads (SEPIA_DRIVER_DIR → platform data dir → PATH). `sepia driver` alone lists them

**Usage:** `sepia driver [COMMAND]`

###### **Commands:**

* `list` — List the sepia-driver-* binaries discovery finds, their probed manifests, and whether each agent command resolves

###### **Options:**

* `-h, --help` — Print help

## `sepia driver list`

List the sepia-driver-* binaries discovery finds, their probed manifests, and whether each agent command resolves

**Usage:** `sepia driver list [OPTIONS]`

###### **Options:**

* `--json` — Print the probed manifests (plus probe problems) as JSON
* `-h, --help` — Print help

## `sepia config`

Agent config IR ops — list/export/import/install/diff rules, skills, commands, hooks, subagents and MCP servers

**Usage:** `sepia config <COMMAND>`

###### **Commands:**

* `list` — List an agent's config items (rules, skills, commands, hooks, subagents, MCP)
* `export` — Export an agent's config as IR JSON (stdout or a file)
* `import` — Install a config IR JSON file into an agent's config store
* `install` — Copy one agent's config into another's store (IR → target's files)
* `diff` — Compare two agents' configs (name-level, plus hook/MCP deltas)

###### **Options:**

* `-h, --help` — Print help

## `sepia config list`

List an agent's config items (rules, skills, commands, hooks, subagents, MCP)

**Usage:** `sepia config list [OPTIONS]`

###### **Options:**

* `--from <FROM>` — Agent to read the config from [possible values: claude, cursor, cline, devin]
* `--claude-dir <CLAUDE_DIR>` — Path to a .claude dir (default: ~/.claude)
* `--cursor-dir <CURSOR_DIR>` — Path to a .cursor dir (default: ~/.cursor)
* `--cline-dir <CLINE_DIR>` — Path to a Cline workspace root (default: cwd)
* `--devin-dir <DEVIN_DIR>` — Path to a Devin config dir (default: ~/.config/devin)
* `-h, --help` — Print help

## `sepia config export`

Export an agent's config as IR JSON (stdout or a file)

**Usage:** `sepia config export [OPTIONS] [OUT]`

###### **Arguments:**

* `[OUT]` — Output file for the config JSON; stdout when omitted

###### **Options:**

* `--from <FROM>` — Agent to read the config from [possible values: claude, cursor, cline, devin]
* `--claude-dir <CLAUDE_DIR>` — Path to a .claude dir (default: ~/.claude)
* `--cursor-dir <CURSOR_DIR>` — Path to a .cursor dir (default: ~/.cursor)
* `--cline-dir <CLINE_DIR>` — Path to a Cline workspace root (default: cwd)
* `--devin-dir <DEVIN_DIR>` — Path to a Devin config dir (default: ~/.config/devin)
* `-h, --help` — Print help

## `sepia config import`

Install a config IR JSON file into an agent's config store

**Usage:** `sepia config import [OPTIONS] <PATH>`

###### **Arguments:**

* `<PATH>` — Config JSON file produced by `sepia config export`

###### **Options:**

* `--to <TO>` — Agent to write the config into [possible values: claude, cursor, cline, devin]
* `--claude-dir <CLAUDE_DIR>` — Path to a .claude dir (default: ~/.claude)
* `--cursor-dir <CURSOR_DIR>` — Path to a .cursor dir (default: ~/.cursor)
* `--cline-dir <CLINE_DIR>` — Path to a Cline workspace root (default: cwd)
* `--devin-dir <DEVIN_DIR>` — Path to a Devin config dir (default: ~/.config/devin)
* `-h, --help` — Print help

## `sepia config install`

Copy one agent's config into another's store (IR → target's files)

**Usage:** `sepia config install [OPTIONS]`

###### **Options:**

* `--from <FROM>` — Agent to read the config from [possible values: claude, cursor, cline, devin]
* `--to <TO>` — Agent to write the config into [possible values: claude, cursor, cline, devin]
* `--claude-dir <CLAUDE_DIR>` — Path to a .claude dir (default: ~/.claude)
* `--cursor-dir <CURSOR_DIR>` — Path to a .cursor dir (default: ~/.cursor)
* `--cline-dir <CLINE_DIR>` — Path to a Cline workspace root (default: cwd)
* `--devin-dir <DEVIN_DIR>` — Path to a Devin config dir (default: ~/.config/devin)
* `-h, --help` — Print help

## `sepia config diff`

Compare two agents' configs (name-level, plus hook/MCP deltas)

**Usage:** `sepia config diff [OPTIONS]`

###### **Options:**

* `--from <FROM>` — Agent to read the config from [possible values: claude, cursor, cline, devin]
* `--to <TO>` — Agent to write the config into [possible values: claude, cursor, cline, devin]
* `--claude-dir <CLAUDE_DIR>` — Path to a .claude dir (default: ~/.claude)
* `--cursor-dir <CURSOR_DIR>` — Path to a .cursor dir (default: ~/.cursor)
* `--cline-dir <CLINE_DIR>` — Path to a Cline workspace root (default: cwd)
* `--devin-dir <DEVIN_DIR>` — Path to a Devin config dir (default: ~/.config/devin)
* `-h, --help` — Print help

## `sepia store`

Store ops — read and write local agent stores directly

**Usage:** `sepia store <COMMAND>`

###### **Commands:**

* `list` — List sessions in a store
* `export` — Export a session from a store — the session IR JSON (default) or Cline session files
* `import` — Import a session (Cline dir, Claude .jsonl, or session JSON) into a store
* `install` — Copy a session into a store ready to resume — cline (default), claude, cursor or devin
* `delete` — Delete a session from a store

###### **Options:**

* `-h, --help` — Print help

## `sepia store list`

List sessions in a store

**Usage:** `sepia store list [OPTIONS]`

###### **Options:**

* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `-h, --help` — Print help

## `sepia store export`

Export a session from a store — the session IR JSON (default) or Cline session files

**Usage:** `sepia store export [OPTIONS] <SESSION_ID> [OUT]`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the store
* `[OUT]` — Output file (or directory) for the session JSON; stdout when omitted — required as a directory for --format cline

###### **Options:**

* `--format <FORMAT>` — Export format: the session IR JSON, or Cline session files [default: json] [possible values: json, cline]
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `-h, --help` — Print help

## `sepia store import`

Import a session (Cline dir, Claude .jsonl, or session JSON) into a store

**Usage:** `sepia store import [OPTIONS] <SOURCE>`

###### **Arguments:**

* `<SOURCE>` — Session to import — a Cline session dir, a Claude .jsonl transcript, or a session JSON export

###### **Options:**

* `--to <TO>` — Store to write the session into (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--session-id <SESSION_ID>` — Override the imported session id
* `--force` — Overwrite a session that already exists in the target store
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia store install`

Copy a session into a store ready to resume — cline (default), claude, cursor or devin

**Usage:** `sepia store install [OPTIONS] <SESSION_ID>`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the source store

###### **Options:**

* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--to <TO>` — Store to write the session into (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--id <ID>` — Session id to install under (defaults to a generated one)
* `--force` — Replace a session that still belongs to a live owner
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia store delete`

Delete a session from a store

**Usage:** `sepia store delete [OPTIONS] <SESSION_ID>`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the store

###### **Options:**

* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia list`

List sessions in a store

**Usage:** `sepia list [OPTIONS]`

###### **Options:**

* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `-h, --help` — Print help

## `sepia export`

Export a session from a store — the session IR JSON (default) or Cline session files

**Usage:** `sepia export [OPTIONS] <SESSION_ID> [OUT]`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the store
* `[OUT]` — Output file (or directory) for the session JSON; stdout when omitted — required as a directory for --format cline

###### **Options:**

* `--format <FORMAT>` — Export format: the session IR JSON, or Cline session files [default: json] [possible values: json, cline]
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `-h, --help` — Print help

## `sepia import`

Import a session (Cline dir, Claude .jsonl, or session JSON) into a store

**Usage:** `sepia import [OPTIONS] <SOURCE>`

###### **Arguments:**

* `<SOURCE>` — Session to import — a Cline session dir, a Claude .jsonl transcript, or a session JSON export

###### **Options:**

* `--to <TO>` — Store to write the session into (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--session-id <SESSION_ID>` — Override the imported session id
* `--force` — Overwrite a session that already exists in the target store
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia install`

Copy a session into a store ready to resume — cline (default), claude, cursor or devin

**Usage:** `sepia install [OPTIONS] <SESSION_ID>`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the source store

###### **Options:**

* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--to <TO>` — Store to write the session into (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--id <ID>` — Session id to install under (defaults to a generated one)
* `--force` — Replace a session that still belongs to a live owner
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia delete`

Delete a session from a store

**Usage:** `sepia delete [OPTIONS] <SESSION_ID>`

###### **Arguments:**

* `<SESSION_ID>` — Session id on the store

###### **Options:**

* `--from <FROM>` — Store to read the session from (devin, cline, claude, cursor) [possible values: devin, cline, claude, cursor]
* `--db <DB>` — Path to the Devin sessions SQLite database [default: /home/luis/.local/share/devin/cli/sessions.db]
* `--data-dir <DATA_DIR>` — Path to the Cline CLI data directory
* `--claude-dir <CLAUDE_DIR>` — Path to the Claude Code data directory (its `projects/` tree is used)
* `--cursor-dir <CURSOR_DIR>` — Path to the Cursor data directory
* `-h, --help` — Print help

## `sepia serve`

Serve the local session plane — ndjson JSON-RPC over stdio (spawned and driven by orchestrators like peregrin)

**Usage:** `sepia serve`

###### **Options:**

* `-h, --help` — Print help

## `sepia version`

Print the sepia version stamp

**Usage:** `sepia version`

###### **Options:**

* `-h, --help` — Print help
