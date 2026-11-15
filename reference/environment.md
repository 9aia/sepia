# Environment

There is no hand-rolled `sepia` binary: the command ships as the
tamias pack in `libraries/sepia/` — a generated `bins[]` shim that
runs `tamias-driver-sepia cli` → `sepia_cli::run_from` in-process
(T-012). Every verb behaves identically through the shim; `sepia
--help` (and the [CLI reference](/reference/cli)) is authoritative —
this page is the env map.

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
| `SEPIA_BIN` | — | path to a `sepia`-shaped executable the view driver spawns for `serve` (the tamias shim, a legacy binary, or a test fake) |

## The driver ops surface

`tamias-driver-sepia` doubles as the generated-surface driver
(P-021). `ops` answers an ops manifest derived from the clap tree:
every bounded verb is a `cli.<verb>` wire query — `driver`, `list`,
`export`, `import`, `install`, `delete`, `config`, `version` and
friends, 18 ops total — dispatched through `sepia_cli::run_from` in
a captured child. `run <argv>` is the raw passthrough; `cli <argv>`
is the `bins[]` shim verb the `sepia` command itself execs (inherited
stdio, so `sepia serve` streams end-to-end). `serve` is listed under
`surfaces`, never an op — it never returns. A feature-gated `mcp`
mode renders the same manifest as MCP tools via pavo-driver-kit.


