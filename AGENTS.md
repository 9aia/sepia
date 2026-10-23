# Sepia

The agent-session abstraction, in Rust — crates plus a `sepia` CLI.
One canonical IR, store drivers, and an ACP client for every agent.
Orchestration across machines lives in peregrin, which spawns
`sepia serve` and drives it over ndjson JSON-RPC stdio.

## Layout

Ports-and-adapters. `sepia-core` is pure domain (no I/O); adapters
depend on it; `sepia-cli` composes the local plane.

| Crate | Role |
| --- | --- |
| `crates/sepia-core` | Session IR, `SessionRepository` port, agent-config IR, restore/rewind planners (pure) |
| `crates/sepia-proto` | `SessionEvent` enum + wire types |
| `crates/sepia-acp` | tolerant ACP client over ndjson stdio |
| `crates/sepia-driver` | driver manifest + ndjson JSON-RPC serve loop |
| `crates/sepia-host` | driver discovery + spawn + respawn |
| `crates/sepia-driver-{devin,cline,claude,cursor}` | store adapter binaries — installed per machine |
| `crates/sepia-session` | local session plane: attach modes, lock probes, prompt/cancel/permission, restore/rewind, `local::build` |
| `crates/sepia-{meta,convert}` | overlay store, store conversion |
| `crates/sepia-telemetry` | opt-in OTEL layer over `tracing` |
| `crates/sepia-cli` | `sepia` binary — store/driver/config verbs + `serve` |
| `crates/sepia-testkit` | conformance suites, golden fixtures, mock ACP agent |

## Boundary

sepia owns *mechanism* — everything you can do **to** a session or
agent. Peregrin owns *policy* — who may act, when, from where (lock
registry, takeover decisions, multi-node merge/route/queue). If a
feature needs a daemon, a multi-machine view, or arbitration between
actors, it belongs to peregrin, not here.

## Commands

```bash
cargo xtask check          # fmt + clippy -D warnings
cargo xtask test           # whole suite (nextest if present)
cargo xtask install        # release-build sepia + drivers → ~/.local
cargo xtask coverage       # llvm-cov report (HTML → target/llvm-cov/html);
                           # extra args pass through, e.g. --fail-under-lines 100
```

Git hooks live in `.hooks/` (`core.hooksPath` = `.hooks`):
pre-commit runs `cargo fmt --check`, pre-push `cargo xtask check`.

## The service wire

`sepia serve` is the embedding surface — ndjson JSON-RPC on stdio, the
same framing as driver binaries (`sepia_driver::rpc`). The method
surface is documented in `crates/sepia-cli/src/serve.rs`; `manifest`
reports `protocol` for version negotiation. New session ops go over
this wire — never link the session plane into an orchestrator.

## Rules

- Workspace lints are strict (pedantic + `unwrap_used` deny-ish in
  `Cargo.toml`); no warnings tolerated. Tests `#![allow]` liberally.
- No `unsafe`. Edition 2024 — `std::env::set_var` needs unsafe, so tests
  spawn real binaries with `.env()` instead.
- Drivers are binaries discovered at runtime — never link a driver into
  the plane; new ops go over the driver wire (`sepia-driver`).
- Golden fixtures in `crates/sepia-testkit/fixtures/` are frozen
  output — regenerate deliberately if the wire changes.
- Blocking I/O (rusqlite, ureq) runs on `spawn_blocking`.
- `.config/nextest.toml` is the test-runner config.

## Faster iteration (community tools, not homemade)

- **sccache**: compile cache — `build.rustc-wrapper = "sccache"` in
  `.cargo/config.toml`.
- **cargo-nextest** (`cargo nextest run -p <crate>`): wired into
  `cargo xtask test` when installed. Scope to one crate.
- **cargo-watch** (`cargo watch -x 'check -p sepia-session'`).
- **cargo-hack**: feature-matrix checks when trimming dependencies.
- **isolated worktree** (`git worktree add --detach <path>`) for big
  rebuilds so your in-progress WIP is untouched.

## Docs

- `docs/session-formats.md`, `docs/protocol.md`,
  `docs/agent-configs.md` — domain references (IR, driver wire, agent
  config IR). Keep them accurate — they feed the docs site.
