# Concepts

The domain vocabulary, each term defined once. Wire shapes live in
[Reference](../reference/).

## Session IR

The canonical intermediate representation every agent store maps
onto: nodes, messages, tool calls, checkpoints, metadata. Lossy
edges are marked `lossy` rather than silently dropped. Shape:
[Session formats](../reference/session-formats.md).

## Store driver

A `sepia-driver-<id>` binary speaking ndjson JSON-RPC over stdio —
the `SessionRepository` for one agent's on-disk store (Devin's
SQLite, Cline's dirs, Claude's JSONL, Cursor's storage). Drivers are
discovered, never linked; see [Architecture](architecture.md#drivers-are-binaries-not-libraries).

## Session plane / ControlPlane

`sepia-session` — the local composition: attach modes, lock probes,
prompt/cancel/permission, restore/rewind, `local::build`. `sepia
serve` exposes it as the service wire.

## `sepia serve`

The embedding surface: ndjson JSON-RPC on stdio. `manifest`
negotiates protocol versions; `sessions.*`, `agents.*`, `config.*`
and the `*.subscribe` feeds cover the plane; `session.event` /
`node.event` notifications stream live events. Full table:
[Serve wire](../reference/serve-wire.md).

## `sessions.prompt_wait`

The blocking turn primitive — prompt → await terminal state →
`PromptWaitResult` (`stopReason`/`text`/`timedOut`). Requests
dispatch concurrently so `cancel`/`permission` interleave with an
in-flight wait. Rides the entity plane as the `sepia.prompt_wait`
op.

## AgentConfig IR

The typed model for agent configuration — skills, rules, commands,
hooks, subagents — that `sepia config` ops and `type: sepia-config`
tamias packs share. See [Agent configs](../reference/agent-configs.md).

## Overlay store (`sepia-meta`)

Sepia-owned per-session metadata — title/pin/projectIds/spans — kept
as a JSON file beside the read-only agent stores. Agent stores are
never written by metadata ops.

## The `sepia` command

A tamias `bins[]` shim over `tamias-driver-sepia` — no hand-rolled
binary. The driver derives an ops manifest from the clap tree
(`cli.<verb>` queries, `run`/`cli` passthroughs) and serves the
pavo views plus an optional MCP surface.
