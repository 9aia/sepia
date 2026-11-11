# Sepia

Coding-agent sessions, agnostically — one canonical IR, store drivers,
and an ACP client for every agent. Rust crates plus a `sepia` CLI.
Devin, Cline, Claude Code, Cursor — more agents are one driver binary
away.

Sepia is the session *abstraction*: it understands every agent's store
format and can manage sessions on a machine — list, load, attach,
prompt, cancel, permissions, restore, rewind — without knowing which
agent produced them. Orchestration across machines (locks, routing,
merged views, the app) is [peregrin](https://github.com/9aia/peregrin)'s
job; peregrin spawns `sepia serve` and drives it over the wire.

## The CLI

```sh
sepia driver list            # sepia-driver-* binaries discovery finds
sepia list                   # sessions in a store
sepia export <id>            # session IR JSON out
sepia install <id> --to claude   # copy a session, ready to resume elsewhere
sepia config list --from claude  # rules/skills/commands/hooks/MCP as IR
sepia serve                  # the session plane, ndjson JSON-RPC on stdio
```

`sepia serve` is the embeddable surface: one process per machine,
spawned and driven like a `sepia-driver-*` binary. Speak ndjson
JSON-RPC on stdin — `manifest`, `agents.list`, `sessions.list`,
`sessions.attach`, `sessions.prompt`, `sessions.subscribe`,
`sessions.restore`, `sessions.rewind`, … — and get events back as
notifications. That's the whole contract an orchestrator needs.

## Install

`sepia` is a tamias pack (`libraries/sepia`) — a generated `bins[]`
shim over `tamias-driver-sepia`, not a cargo-installed binary:

```sh
tamias install 9aia:sepia@0.1.0    # org registry (needs TAMIAS_REGISTRY_TOKEN)
```

From a checkout: `cargo xtask pack` stages the driver payload, then
`tamias install --allow-drivers ./libraries/sepia` deploys the shim.
See `## Develop` below.

## Concepts

**Session IR** — the canonical representation every store reads and
writes (`sepia-core`): messages with roles and content blocks (text,
thinking + signature, tool calls, images, file attachments), tool calls
with args/results/status/diffs, token usage, sub-agent lineage,
checkpoints, and a meta overlay (title/pin/projects/spans).

**Drivers** — store adapters as separate binaries
(`sepia-driver-devin`, `sepia-driver-cline`, `sepia-driver-claude`,
`sepia-driver-cursor`). Discovered at runtime from
`$SEPIA_DRIVER_DIR` → the platform data dir →
`/usr/local/lib/sepia/drivers` → PATH, probed for a
manifest + capabilities (SessionStore, SessionWrite, Checkpoints,
Restore, Rewind, AgentConfig), and driven over ndjson JSON-RPC stdio.

**ACP** — agents attach through the Agent Client Protocol
(`sepia-acp`): spawn, initialize/capabilities, session
list/load/prompt/cancel, normalized update streams, permission
round-trips. The wire boundary is tolerant `serde_json::Value`.

**Session plane** — `sepia-session`: live-session ownership, lock
probing, attach/takeover mechanics, prompt/cancel/permission routing,
restore/rewind execution, plus `local::build` — the composition that
assembles it all from this machine's drivers and agent CLIs.

## Layout

| Crate | Role |
| --- | --- |
| `crates/sepia-core` | Session IR, `SessionRepository` port, agent-config IR, restore/rewind planners (pure) |
| `crates/sepia-proto` | `SessionEvent` enum + wire types |
| `crates/sepia-acp` | tolerant ACP client over ndjson stdio |
| `crates/sepia-driver` | driver manifest + ndjson JSON-RPC serve loop |
| `crates/sepia-host` | driver discovery + spawn + respawn |
| `crates/sepia-driver-{devin,cline,claude,cursor}` | store adapter binaries |
| `crates/sepia-session` | the local session plane: attach/takeover mechanics, prompt/cancel/permission, restore/rewind, `local::build` composition |
| `crates/sepia-meta` | overlay store (title/pin/projects/run spans) |
| `crates/sepia-convert` | store format conversion |
| `crates/sepia-telemetry` | opt-in OTEL layer over `tracing` |
| `crates/sepia-peregrin` | entity-plane adapter — sessions as `kind="sepia-session"` entities |
| `crates/sepia-cli` | the `sepia` engine lib (`command`/`run_from`) — no bin; the command ships as the tamias pack |
| `crates/sepia-driver-pavo` | `tamias-driver-sepia` — pavo's read-only view driver over `sepia serve`, plus the engine ops surface (`ops`/`cli.*`/`run`, `cli` = the bins-shim engine passthrough, `mcp` feature-gated) |
| `libraries/sepia` | the tamias pack — `tamias.yaml` declares `bins: [{name: sepia, shim: cli, driver: sepia}]`; `cargo xtask pack` stages `drivers/bin/<triple>/tamias-driver-sepia` (gitignored) |
| `crates/sepia-testkit` | conformance suites + golden fixtures |

## Develop

```bash
cargo xtask check    # fmt + clippy -D warnings
cargo xtask test     # whole suite (nextest if present)
cargo xtask pack     # stage libraries/sepia driver payload
                     # (release tamias-driver-sepia → drivers/bin/<triple>/)
cargo xtask install  # release-build tamias-driver-sepia + store
                     # drivers → ~/.local (the `sepia` command itself
                     # is the tamias pack's generated bins[] shim —
                     # `cargo xtask pack` + `tamias install
                     # --allow-drivers ./libraries/sepia`)
```

## Docs

The `docs/` worktree holds the VitePress site (Diátaxis:
tutorials, how-to, reference, explanation) — the session IR and
per-agent store formats in `reference/session-formats.md`, the driver
wire in `reference/protocol.md`, the `sepia serve` surface in
`reference/serve-wire.md`, and the agent config IR in
`reference/agent-configs.md`.

[CONTRIBUTING](CONTRIBUTING.md) · [SECURITY](SECURITY.md) ·
[LICENSE](LICENSE) (MIT) · [project](project/index.md) — TODO, backlog
