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

<!-- crate-map -->
| crate | layer | purpose |
|---|---|---|
| `sepia-core` | domain | Sepia session IR and domain ports — pure domain, zero I/O deps |
| `sepia-proto` | domain | Protocol v2 wire types — REST payloads, SSE events, shared models |
| `sepia-acp` | adapter | ACP client — spawn agents over stdio, typed session ops, tolerant update normalization |
| `sepia-driver` | adapter | SDK for sepia store drivers — manifest, ndjson JSON-RPC serve loop, fs helpers |
| `sepia-driver-claude` | adapter | Claude Code store driver — ~/.claude/projects/*.jsonl transcripts as a SessionRepository over the driver-wire protocol |
| `sepia-driver-cline` | adapter | Cline store driver — ~/.cline/data session dirs as a SessionRepository over the driver-wire protocol |
| `sepia-driver-config` | adapter | tamias-driver-sepia-config — `type: sepia-config` content driver: agent-config IR packs deployed to agent config dirs. |
| `sepia-driver-cursor` | adapter | Cursor store driver — ~/.cursor chats store.db + agent-transcripts as a SessionRepository over the driver-wire protocol |
| `sepia-driver-devin` | adapter | Devin store driver — sessions.db (sqlite) SessionRepository over the driver-wire protocol |
| `sepia-host` | adapter | Daemon side of the sepia driver protocol — discovery, spawn, registry, remote SessionRepository |
| `sepia-http` | adapter | Sepia's session REST surface — axum adapter mounting /api/sessions*, agents, config/projects compat, feed and meta wiring over SessionState |
| `sepia-peregrin` | adapter | Sepia's first-class peregrin adapter — sessions as `sepia-session` entities over the entity plane |
| `sepia-convert` | service | Cross-store session conversion — history→IR rebuild, Devin-store import with cog grafting |
| `sepia-meta` | service | Sepia-owned per-session overlay metadata (title/pin/projectIds/spans) — a JSON file beside the read-only session stores |
| `sepia-session` | service | Sepia session plane — live-session ownership, lock probes, attach/takeover, prompt/cancel/permission, restore/rewind, and the machine-local composition |
| `sepia-cli` | engine | The sepia engine lib — local store verbs (list/export/import/install/delete), agent config IR ops, driver discovery, and the stdio session-plane service; the `sepia` command ships as a tamias bins[] shim over this lib |
| `sepia-driver-pavo` | driver | tamias-driver-sepia — pavo view driver over `sepia serve` (read-only session/agent surfaces) plus engine ops dispatching to the sepia-cli lib |
| `sepia-testkit` | testkit | Sepia test backbone — conformance suites, golden fixtures, mock drivers/agents |
| `xtask` | tooling | sepia-xtask — workspace automation: check / test / coverage / pack / install / fmt. |

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
versions; `sessions.*`, `agents.*`, `config.*`, and the
two `*.subscribe` feeds cover the plane's surface; `session.event` and
`node.event` notifications stream live events. The full method table
lives in [serve-wire](../reference/serve-wire.md) and
`crates/sepia-cli/src/serve.rs`.

## Where it sits

In the org's layering — **delivery** (tamias distribution, scarab
config/templates, tarsi releases, bupha CI, hypha entity sync) →
**memory** (myco records, kuda code maps) → **process** (sturn
coordination, cuora secrets, mimus proofs, cicada schedules, lasius
workspace, **sepia** sessions, peregrin campaigns) → **interface**
(pavo views, MCP fronts).

Sepia is a **process-layer organism** — the session mechanism every
orchestrator and surface builds on. Peregrin (process) spawns
`sepia serve`; pavo and the ops manifest read through it;
`sepia-peregrin` projects sessions onto hypha's entity plane as
`kind="sepia-session"` entities. The `sepia` command itself ships
as a tamias pack.
