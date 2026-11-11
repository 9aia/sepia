# Architecture

Sepia is the *mechanism* layer for coding-agent sessions. Everything
you can do **to** a session or an agent — list stores, attach, prompt,
cancel, resolve permissions, restore, rewind, manage agent configs —
is sepia's job. It has no opinion about *who* should do those things,
*when*, or *from where*: that policy lives in
[peregrin](https://9aia.com/peregrin/).

```
orchestrator (peregrin, scripts, CI)
        │  spawn + ndjson JSON-RPC
        ▼
   sepia serve ────────── ControlPlane (sepia-session)
        │                       │ attach / prompt / restore
        │  spawn + ndjson       ▼
        └────────────►  sepia-driver-* binaries
                                │ per-agent adapters
                                ▼
                     devin / cline / claude / cursor stores
```

## Why mechanism vs policy

Two failure modes drove the split. Policy code (arbitration, routing,
queues) wants to evolve fast and is mostly pure logic. Session
mechanism wants to be boring — a stable IR, tolerant parsers, careful
handling of agent stores you did not design. Gluing them together made
both worse; the `sepia serve` boundary keeps them apart: peregrin
links none of sepia's internals, it spawns the service and speaks the
same JSON-RPC framing drivers do.

## The canonical IR

Every agent store — Devin's SQLite, Cline's directory layout, Claude's
JSONL transcripts, Cursor's workspace storage — maps onto one session
IR: nodes, messages, tool calls, checkpoints, metadata. Lossy edges are
marked `lossy` in metadata rather than silently dropped, so importers
and auditors can see what a store could not express.
[Session formats](../reference/session-formats.md) documents the shape.

## Drivers are binaries, not libraries

Store adapters ship as `sepia-driver-<id>` executables speaking ndjson
JSON-RPC over stdio. Discovery order: `$SEPIA_DRIVER_DIR`, the platform
data dir (`$XDG_DATA_HOME/sepia/drivers` or `~/.local/share/sepia/drivers`),
`/usr/local/lib/sepia/drivers`, then PATH. The session plane never links
a driver — a crashed or hung adapter kills a subprocess, not your
daemon. Adding an agent is dropping a binary in a discovery dir; see
[write a driver](../how-to/write-a-driver.md).

## Crates

| Crate | Role |
| --- | --- |
| `sepia-core` | Session IR, `SessionRepository` port, agent-config IR, restore/rewind planners — pure, no I/O |
| `sepia-proto` | `SessionEvent` enum and wire types |
| `sepia-acp` | Tolerant ACP client over ndjson stdio |
| `sepia-driver` | Driver manifest + the serve loop drivers run |
| `sepia-host` | Driver discovery, spawn, respawn |
| `sepia-driver-{devin,cline,claude,cursor}` | The per-agent adapter binaries |
| `sepia-session` | The local session plane — attach modes, lock probes, prompt/cancel/permission, restore/rewind |
| `sepia-meta`, `sepia-convert` | Overlay store; cross-store conversion/import |
| `sepia-telemetry` | Opt-in OTEL layer over `tracing` |
| `sepia-peregrin` | The entity-plane adapter — sessions as `kind="sepia-session"` entities (`/api/entities*` client + server side) |
| `sepia-cli` | The `sepia` engine lib (`command`/`run_from`) — **no bin**: the command ships as the tamias pack |
| `sepia-driver-pavo` | `tamias-driver-sepia` — pavo's view driver over `sepia serve`, plus the engine ops surface (`ops` manifest, `cli.*` queries, `run`/`cli` passthroughs) |
| `sepia-testkit` | Conformance suites, golden fixtures, mock ACP agent |

## Distribution: the tamias pack

There is no hand-rolled `sepia` binary. The command ships as the
tamias pack in `libraries/sepia/` — `tamias.yaml` declares
`bins: [{name: sepia, shim: cli, driver: sepia}]`, so
`tamias install 9aia:sepia@<ver>` (or `./libraries/sepia` from a
checkout) deploys a generated shim that execs
`tamias-driver-sepia cli <argv>` → `sepia_cli::run_from` in-process.
Full argv parity, `sepia serve` included — the unbounded wire loop
rides the same shim as a *surface*, never an op.

`tamias-driver-sepia` (the `sepia-driver-pavo` crate) is also the
generated-surface driver: its `ops` manifest derives from
`sepia_cli::command()` — every bounded verb becomes a `cli.<verb>`
wire query (18 ops today), `run <argv>` re-execs the engine, and a
feature-gated `mcp` renders the same manifest as MCP tools. The pavo
views proxy `sepia serve` read-only on top of that. See
[CLI and environment](../reference/cli.md#the-driver-ops-surface).

## The service wire

`sepia serve` is the embedding contract. `manifest` negotiates protocol
versions; `sessions.*`, `agents.*`, `projects.*`, `config.*`, and the
two `*.subscribe` feeds cover the plane's surface; `session.event` and
`node.event` notifications stream live events. The full method table
lives in [serve-wire](../reference/serve-wire.md) and
`crates/sepia-cli/src/serve.rs`.
