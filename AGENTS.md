# Sepia

A Rust workspace — one UI for coding-agent sessions across machines
(node daemons + a Leptos hub). `README.md` has the user-facing story;
`DEPLOY.md` has install/env; `project/plans/rust-rewrite.md` is the
design doc.

## Layout

Ports-and-adapters. `sepia-core` is pure domain (no I/O); adapters
depend on it; `sepia-node`/`sepia-cli`/`sepia-hub` compose ports.

| Crate | Role |
| --- | --- |
| `crates/sepia-core` | Session IR, `SessionRepository` port, restore/rewind planners (pure) |
| `crates/sepia-proto` | `SessionEvent` enum + wire types |
| `crates/sepia-driver-{sdk,host}` | driver serve loop / discovery + spawn + respawn (ndjson JSON-RPC) |
| `crates/sepia-driver-{devin,cline,claude,cursor}` | store adapter binaries — installed per machine |
| `crates/sepia-acp` | tolerant ACP client over ndjson stdio |
| `crates/sepia-control` | control plane: merged lists, attach/takeover/locks, prompt/cancel/permission, restore/rewind |
| `crates/sepia-{meta,convert,outbox,push,sync}` | overlay store, conversion, durable write queue, web-push, node→hub projection |
| `crates/sepia-http` | axum REST + SSE (`env.rs` documents every `SEPIA_*` var) |
| `crates/sepia-node` | headless daemon binary |
| `crates/sepia-cli` | `sepia` binary — store/node/config verbs, serve, pair, service |
| `crates/sepia-{web,hub}` | Leptos UI + SSR host |
| `crates/sepia-testkit` | conformance suites, golden fixtures, mock ACP agent |

## Commands

```bash
cargo xtask check          # fmt + clippy -D warnings
cargo xtask test           # whole suite (nextest if present)
cargo xtask install        # release-build binaries → ~/.local/bin
cargo leptos watch         # hub dev loop (wasm → target/site)
```

Git hooks live in `.hooks/` (`core.hooksPath` = `.hooks`):
pre-commit runs `cargo fmt --check`, pre-push `cargo xtask check`.

## Rules

- Workspace lints are strict (pedantic + `unwrap_used` deny-ish in
  `Cargo.toml`); no warnings tolerated. Tests `#![allow]` liberally.
- No `unsafe`. Edition 2024 — `std::env::set_var` needs unsafe, so tests
  spawn real binaries with `.env()` instead (see
  `crates/sepia-node/tests/node.rs`).
- Drivers are binaries discovered at runtime — never link a driver into
  the node; new ops go over the driver wire (`sepia-driver-sdk`).
- Golden fixtures in `crates/sepia-testkit/fixtures/` are frozen TS
  output — regenerate via `cargo xtask fixtures` if the wire changes.
- Blocking I/O (rusqlite, ureq) runs on `spawn_blocking`; `NodeClient`
  is blocking by design.
- `.config/nextest.toml` is the test-runner config.

## Docs

- `docs/session-formats.md`, `docs/protocol.md`, `docs/agent-configs.md`
  — domain references (written for the TS tree; still accurate on IR +
  wire semantics).
