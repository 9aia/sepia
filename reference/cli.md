# CLI and environment

The `sepia` binary's command surface and the environment variables the
plane reads. `sepia --help` is authoritative; this page is the map.

## Commands

Top-level store verbs are aliases for the `store` namespace — `sepia
list` ≡ `sepia store list`, and so on.

| Command | What it does |
| --- | --- |
| `sepia driver list [--json]` | Discovery: the `sepia-driver-*` binaries found, their probed manifests, and whether each agent command resolves. `sepia driver` alone lists them. |
| `sepia list` | List sessions in a store (`store list`). |
| `sepia export <id> [out] [--format json\|cline]` | Session IR JSON to stdout/a file; `--format cline` writes Cline session files instead (out is then a directory). |
| `sepia import <source> [--to <agent>] [--session-id <id>] [--force]` | Import a Cline session dir, a Claude `.jsonl` transcript, or a session JSON export into a store. |
| `sepia install <id> [--from <a>] [--to <b>] [--id <id>] [--force]` | Copy a session into a store ready to resume — `cline` (default), `claude`, `cursor`, or `devin`. |
| `sepia delete <id> [--from <agent>]` | Delete a session from a store. |
| `sepia config list\|export\|import\|install\|diff` | Agent config IR ops — rules/skills/commands/hooks/subagents/MCP. See [agent configs](agent-configs.md). |
| `sepia serve` | The session plane — ndjson JSON-RPC on stdio. See [serve wire](serve-wire.md). |
| `sepia version` | Version stamp. |

Store flags (`--devin-dir`, `--claude-dir`, `--cursor-dir`,
`--cline-dir`) pick the store directory and imply the agent when
`--from`/`--to` is omitted.

## Environment

Read by `sepia serve` and the store verbs; driver discovery and store
paths apply to the `serve` child, so set them in its environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `SEPIA_HOME` | platform data dir | sepia's own state dir (`node.json`, meta overlay) |
| `SEPIA_NODE` | — | node identity written into `node.json` |
| `SEPIA_NAME` | — | display name for this node |
| `SEPIA_DRIVER_DIR` | — | extra driver discovery dir (first in order) |
| `SEPIA_CLAUDE_DIR` | `~/.claude` | Claude Code config/session dir |
| `SEPIA_CURSOR_DIR` | `~/.cursor` | Cursor dir |
| `SEPIA_CLINE_DIR` | cwd | Cline dir |
| `SEPIA_DEVIN_DIR` | platform data dir | Devin CLI dir |
| `SEPIA_DEVIN_DB` | `<devin dir>/sessions.db` | Devin sessions SQLite path |
| `SEPIA_DEVIN_READONLY` | off | open the Devin DB read-only |
| `SEPIA_AGENT_<ID>_COMMAND` | — | override the agent CLI command per driver (e.g. `SEPIA_AGENT_CLAUDE_COMMAND`) |
| `SEPIA_EVENT_BUFFER` | — | session-event buffer size |
| `SEPIA_IDLE_TTL_MS` | — | idle session TTL |
| `SEPIA_SWEEP_MS` | — | sweeper interval |
| `SEPIA_LOCK_TTL_MS` | — | session lock TTL |
| `SEPIA_HISTORY_LIMIT` | — | prompt-history cap |
| `SEPIA_OTEL_ENDPOINT` | — | OTLP endpoint (opt-in telemetry) |
| `SEPIA_OTEL_SERVICE_NAME` | — | OTEL service name |
| `SEPIA_INHERIT_ENV` | — | env vars passed through to agent subprocesses |
| `SEPIA_DEBUG` | — | debug logging |
| `SEPIA_BIN` | — | path to the `sepia` binary (used by `sepia-driver-pavo`) |
