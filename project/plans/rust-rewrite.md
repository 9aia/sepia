# Sepia → Rust: full rewrite plan

Status: in execution on `rust-rewrite`. Landed: everything through
Phase 5 core — workspace + lints, golden fixtures, driver SDK/host +
discovery + respawn, sepia-core IR/ports, all four drivers
(contract+golden green), tolerant sepia-acp, control plane (lock/
takeover/idle-sweep + fault-injection + soak suites), sepia-meta +
sepia-convert, rewind over the driver wire, sepia-node serving real
HTTP end-to-end (real driver + mock agent e2e, RSS budget), sepia-http
full REST+SSE surface, sepia-cli (full verb surface), sepia-outbox,
sepia-push, sepia-sync (projection + outbox drain + reconnect,
subscribe-before-list), sepia-web Leptos UI (list/detail + agents/
projects/nodes/settings pages, SSR + hydrate-checked wasm), sepia-hub
(sync-backed SyncNodeApi, session-routed proxies, hub-owned push store

- feed-marker fan-out, PWA assets). Hub↔node capstone e2e passes.
  README + DEPLOY rewritten. In flight: session-detail interactive ops
  UI. Pending: browser e2e (deferred — thirtyfour vs playwright call),
  TS deletion (Phase 6 final step).
  Scope: **everything** — core IR, store adapters, ACP client, control plane,
  server, CLI, UI, release tooling. TypeScript deleted at the end.
  This is simultaneously a **rewrite and a redesign**: keep the phases
  decoupled so the redesign never pulls the port forward.

## Decisions (locked)

- **Topology: hybrid federation.** Nodes stay authoritative for their agent
  stores and all live agent control. A central projection is an
  _optimization only_ — a materialized view, never an authority.
- **Client: Leptos SSR + hydrate + WASM + PWA** served by `sepia-hub`.
  Phone = installed PWA: offline app shell (cached last projection,
  read-only + outbox visibility), push notifications via `web-push`.
- **Writes to offline nodes QUEUE.** The hub owns a durable outbox
  (SQLite): mutations persist locally with `queued` state and drain in
  per-session FIFO order when the node returns. Idempotency keys make
  retry safe; TTL + dead-letter for ops that can never apply.
- **Local-first, but honest about it**: "local SQLite" is the hub's
  embedded projection + outbox DB. No PowerSync/Electric (both require
  Postgres logical replication on the source; our sources are agent-owned
  files). No browser-side sqlite-wasm tier.
- **Drivers are subprocesses**, not bundled crates — install only what the
  machine has.
- **AG-UI dropped entirely** — including `POST /api/agent`. UI consumes
  `sepia-proto` events directly (ACP → proto → UI).
- **No legacy code carried over.** Fresh protocol (v2 — we are NOT bound
  to wire-compat with the Bun server), fresh routes, fresh event shapes.
  Consequence: the React-UI-as-regression-harness gate dies — replaced by
  testkit contract suites + golden IR fixtures + Rust e2e.
- **ACP is load-bearing** — `agent-client-protocol` crate (v3) is a core dep.
- **mac + linux only** (arm64 + x86_64; linux gnu + musl static).

## Deployment model

```text
laptop B/C                      main machine (laptop A)            phone
┌─────────────┐                ┌──────────────────┐              ┌────────┐
│ sepia-node  │  ────────────► │ sepia-node       │              │ browser│
│ (headless)  │   protocol v2  │ sepia-hub        │ ◄── SSR/SSE ─┤ (WASM) │
└─────────────┘                │ (Leptos + sync)  │              └────────┘
                               └──────────────────┘
```

Three binaries, three dependency graphs — nothing shares weight it
doesn't need:

- **`sepia-node`** — headless daemon: axum + driver host + ACP + control
  plane. No Leptos, no sqlx, no UI deps. "Daemon-only" machines install
  just this.
- **`sepia-hub`** — Leptos SSR host + SQLite projection + node sync.
  Installs where the UI lives; runs alongside a node on the same machine.
- **`sepia`** — CLI: store/node/config/pair/service verbs over the
  protocol.

## Why no generic sync engine

Every session has exactly **one authoritative node**, so the hub projection
is a materialized view with cursors — not a replicated-writes problem.
`sepia-sync` = per-node `updatedAt` cursors + delta `GET /sessions?since=`

- `/api/events` SSE + periodic full-list reconcile for tombstones.
  `sepia-outbox` handles the write direction: queued commands are
  idempotent (client-supplied keys), ordered per session, and surface
  failure honestly rather than merging — a prompt that arrives at a now-
  busy node fails into dead-letter, it doesn't interleave. Don't buy
  generic sync infrastructure; the problem is smaller than that.

**Constraint: the projection holds indexes only** — session summaries,
titles, metadata. Message bodies are the bulk of the data; eagerly syncing
them recreates the problem local-first avoids. History stays node-fetched
with a hub-side cache.

## Crate map

```text
Cargo.toml                  # workspace root: members, workspace.lints, workspace.dependencies
rust-toolchain.toml         # pinned channel + rustfmt/clippy/llvm-cov components
rustfmt.toml / clippy.toml / deny.toml / .config/nextest.toml

crates/
  sepia-core/               # pure domain: IR types, ports (traits), Restore/Rewind.
                            # ZERO I/O deps — no tokio, no fs. Compiles to wasm32.
  sepia-proto/              # wire types: protocol v2 REST + event stream + ACP-facing models.
                            # THE contract crate — shared by node, hub, web, testkit.
  sepia-driver-sdk/         # plugin SDK: Driver trait, capability traits, manifest types,
                            # ndjson JSON-RPC server loop for driver binaries
  sepia-driver-host/        # daemon side: discovery, spawn, supervise, RPC client,
                            # capability registry
  sepia-driver-devin/       # driver binary: sessions.db via rusqlite (read-only)
  sepia-driver-cline/       # driver binary: fs dirs
  sepia-driver-claude/      # driver binary: projects/*.jsonl
  sepia-driver-cursor/      # driver binary: store.db + transcripts
  sepia-convert/            # cross-store conversion, composes drivers via ports
  sepia-acp/                # ACP client on agent-client-protocol v3 (process feature)
  sepia-control/            # control plane: live-session ownership, locks, idle TTL,
                            # attach/takeover, prompt/cancel/permission
  sepia-sync/               # node→hub projection sync (cursors, deltas, tombstones)
  sepia-outbox/             # durable mutation queue: per-session FIFO, idempotency
                            # keys, retry budget, TTL, dead-letter surfacing
  sepia-http/               # axum driving adapter: REST + SSE, auth, pairing,
                            # gateway proxy, transfer, push
  sepia-node/               # daemon binary (thin: wires http+control+driver-host)
  sepia-hub/                # Leptos SSR host binary + projection store (rusqlite)
  sepia-web/                # Leptos UI lib (SSR + hydrate → wasm)
  sepia-cli/                # clap binary
  sepia-testkit/            # conformance suites + fixtures + mock ACP agent
xtask/                      # cargo xtask check|test|golden|e2e|coverage|dist
```

Enforced dependency rule: **nothing outside an adapter crate may name
`rusqlite`, `tokio::fs`, `std::net`, `hyper`, or `agent_client_protocol`'s
I/O features.** Checked by convention + `cargo-deny` bans. `sepia-core`
must compile to `wasm32-unknown-unknown` unchanged — one IR serves node,
hub, and browser.

## Driver/plugin architecture

Each adapter is a standalone binary (`sepia-driver-<name>`) speaking a
versioned ndjson JSON-RPC protocol over stdio — the same wire machinery
ACP already requires, so no second transport stack. Non-Rust drivers can
implement the protocol later.

```rust
pub struct DriverManifest {
    pub id: DriverId,                         // "devin", "cline", ...
    pub version: semver::Version,
    pub protocol: u32,                        // driver-wire protocol version
    pub capabilities: BTreeSet<Capability>,
    pub agent_command: Option<String>,        // e.g. "cline --acp"
    pub config_schema: serde_json::Value,     // JSON Schema for driver config
}

pub enum Capability {
    SessionStore,        // list/history/export
    Checkpoints,
    Restore,             // file restore
    Rewind,              // transcript rewind
    Convert { from: DriverId, to: DriverId },
}
```

**Discovery order**: `$SEPIA_DRIVER_DIR` → `~/.local/share/sepia/drivers/`
→ `/usr/local/lib/sepia/drivers/` → PATH scan for `sepia-driver-*`.
Manifest is a sibling `<name>.driver.toml`.

**Agent visibility**: the control plane advertises an agent only when
_both_ its driver binary exists _and_ `agent_command` resolves on PATH —
e.g. `claude` appears iff `sepia-driver-claude` + `claude-agent-acp` are
both present. This is the "agent itself + integration crate both
discoverable" requirement.

**Consumers never name drivers**: `registry.with(Capability::SessionStore)`
→ merged repository; `registry.driver_for(session.agent)` for targeted
ops. Adding a fifth agent = installing a driver; zero upstream changes.

Rejected alternative: Extism/WASM plugins — drivers need sqlite +
arbitrary fs, which means hand-rolling host functions; subprocess gives
the same sandboxing story with zero ABI problems.

## Library choices (beyond the crate map)

| Need           | Choice                                               | Note                                                        |
| -------------- | ---------------------------------------------------- | ----------------------------------------------------------- |
| ACP            | `agent-client-protocol` v3                           | transport/lifecycle only — see strictness warning below     |
| HTTP server    | `axum` on `tokio`                                    | SSE via `axum::response::Sse` + bounded `broadcast`         |
| Node→peer HTTP | `hyper-util` / `ureq`                                | not `reqwest` on the node — keep the daemon lean            |
| DB             | `rusqlite` everywhere                                | sync, small; no sqlx                                        |
| Errors         | `thiserror` in libs, `anyhow` only at binary edges   |                                                             |
| Validation     | serde + `validator`                                  | replaces Effect Schema                                      |
| Async control  | `tokio::task::JoinSet` + `CancellationToken`         | replaces Effect structured concurrency                      |
| Telemetry      | `tracing` + `tracing-opentelemetry`                  | replaces OTEL SDK wiring                                    |
| Push           | `web-push` crate                                     | VAPID                                                       |
| SSH tunnels    | `russh` or keep `ssh` subprocess                     | decide at port time                                         |
| Markdown       | `comrak`                                             | not incremental — render per completed block when streaming |
| Highlight      | `syntect`                                            | tree-sitter only if code-aware features need it             |
| UI components  | Thaw base + hand-built tree/palette/dnd/virtual-list | see UI risk                                                 |
| Paths          | `camino::Utf8Path`                                   | UTF-8 correctness through serde                             |
| Dates          | `jiff` or `chrono`                                   | epoch-ms interop with stores                                |
| UUID/crypto    | `uuid`, `sha2`, `subtle`, `rand`                     | pairing/token paths                                         |

### ACP strictness warning

`normalize.ts` deliberately coerces messy real-world agent output that
`devin acp`/`cline --acp` actually emit. The Rust SDK is strongly typed
and will reject what the TS code tolerated. **Use the SDK for transport
and lifecycle; keep `serde_json::Value` at the message boundary and port
the tolerant normalizer.** Otherwise the port is stricter than production
— a regression, not a bug.

## Effect → Rust translation rules

`Effect<A, E>` → `async fn → Result<A, thiserror>`; `Layer` → constructor
returning `Arc<dyn Trait>`; `Schema` → serde derives + `validator`;
`Option` → `Option`; `Metric.counter` → tracing/otel; `Scope`/finalizers →
`Drop` + `CancellationToken`; structured concurrency → `JoinSet`.

## Toolchain / quality gates

| Need           | Tool                                                                                                                                   | Enforced by                                       |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| fmt            | `rustfmt` + `cargo fmt --check`                                                                                                        | `xtask check`, CI                                 |
| lint           | `clippy` via workspace `[lints]`: correctness+suspicious+perf deny, pedantic warn, `unwrap_used`/`expect_used` warn (allowed in tests) | `clippy --workspace --all-targets -- -D warnings` |
| typecheck      | `cargo check --all-targets`                                                                                                            | per-save                                          |
| tests          | `cargo nextest run`                                                                                                                    | parallelism, per-test timeout                     |
| coverage       | `cargo-llvm-cov`                                                                                                                       | `xtask coverage`, per-crate floors, raise-only    |
| deps           | `cargo-deny` (licenses/advisories/bans) + `cargo-hakari`                                                                               | CI                                                |
| property tests | `proptest`                                                                                                                             | IR round-trips, normalizer, codecs                |
| snapshots      | `insta`                                                                                                                                | golden IR/wire fixtures                           |
| automation     | `cargo xtask`                                                                                                                          | single entrypoint                                 |
| packaging      | `cargo-dist`: mac arm64+x86_64, linux gnu+musl                                                                                         | release CI                                        |

CI matrix: `macos-14` + `ubuntu-latest` × stable. `rust-toolchain.toml`
pins channel; `Cargo.lock` committed.

## Testing pyramid (behavior-first)

```text
            e2e: full binaries + real stores + real ACP agents
           /   \      (gated like today's ACP_IT=1; xtask e2e)
   integration: axum in-process, SSE streams, hub sync, driver lifecycle
         /        \
  contract: sepia-testkit conformance suites
       /              \
 unit + proptest: pure domain, normalizers, codecs
```

**`sepia-testkit` is the backbone** — behavioral contracts run against any
implementation:

- `assert_session_repository_contract(store)` — pagination, ordering, IR
  invariants, error taxonomy. Each driver's test = build fixture store →
  run suite. New drivers get full contract coverage for free.
- `assert_http_contract(server)` — the v2 surface defined in
  `sepia-proto`; behavior equivalence to the old API is checked via
  golden fixtures, not wire parity.
- `assert_driver_contract(driver)` — manifest sanity + capability wiring.
- `assert_outbox_contract(store)` — FIFO ordering, idempotent retry,
  TTL/dead-letter, no interleaving across sessions.
- Mock ACP agent built with `agent-client-protocol`'s **agent-side** API —
  control-plane tests with zero JS.
- Golden fixtures in `sepia-testkit/fixtures/` (extracted from the TS
  adapters in Phase 0): Rust IR output must match byte-for-byte.
- `proptest`: IR serde round-trips, `normalize_update` on arbitrary ACP
  JSON, frontmatter, SSE codecs.
- **Fault injection** (integration tier): kill agent mid-turn, driver
  crash mid-RPC, partial/corrupt JSON-RPC lines, SSE drop+reconnect
  storms, node down during outbox drain. Scripted in testkit, not ad hoc.
- **Soak test** (`xtask soak`, CI-nightly not per-commit): N hours of
  attach/detach/prompt churn against the real daemon; assert stable RSS,
  fd count, and task count. Leaks are the classic long-daemon bug class.
- **`loom`** on the control-plane lock-probe state machine — the highest
  race-risk code in the system.
- **Browser e2e**: `thirtyfour`/`fantoccini` (WebDriver) for the PWA —
  playwright dies with the TS workspace. If it proves too clunky, the
  fallback is a minimal playwright suite retained as the ONLY remaining
  TS; decide at Phase 5.
- `criterion` micro-benches + latency budgets: session list @10k rows,
  history page, IR export — budgets set in Phase 1, regression-gated.

"100% tested" is framed as **conformance suites + coverage floors +
property tests on pure code + fault injection** — not literal 100% on
timing-sensitive paths, which buys flaky CI, not quality.

## Resilience rules (codified)

- Every network call inside `tower::timeout`, per-route budgets; retries
  only on idempotent GETs, exponential backoff + jitter.
- Every spawned child (drivers, ACP agents, ssh) supervised: kill-on-drop
  guard, stderr ring buffer, exit → typed `ControlError`.
- `CancellationToken` through every long-lived op; `JoinSet` for fan-out
  (the `session/list` lock-probe merge).
- SSE: bounded `broadcast` (lagged → resync marker, never block),
  keepalive ticker, per-connection metering.
- Node failures degrade to partial data, never fail the aggregate list.
- No unbounded collections anywhere: streaming pagination end-to-end,
  LRU on caches, capped prompt/attachment bodies.
- Outbox durability: SQLite WAL + synchronous=NORMAL minimum; a queued
  write acknowledged to the UI must survive a hub restart (outbox is NOT
  disposable like the projection).
- Destructive ops keep their gates: restore/rewind require explicit
  confirm tokens (carried semantics, fresh implementation) and are
  refused while a session is busy or held.

## Security model (was missing — now specified)

- **Single user, three credential tiers**: node bearer token (as today),
  hub session (browser login → httpOnly cookie; no bearer tokens in
  `localStorage` — kills the deferred TODO.md item), device pairing for
  new browsers.
- **Hub stores node credentials server-side** in the OS keychain
  (`keyring` crate; mac/linux both supported), SQLite fallback only if
  keyring unavailable + documented.
- CSRF tokens on hub mutations; SSE auth via short-lived ticket, not
  `?access_token=` in URLs.
- TLS terminates at the existing Caddy/reverse-proxy layer; hub and
  nodes stay HTTP on loopback/LAN unless `SEPIA_HOST` demands otherwise
  (token required off-loopback, as today).
- Port the TODO.md deferred items deliberately: `POST /api/pair`
  per-IP rate limit (yes — cheap), gateway SSRF per-request resolver
  check (yes — it was already designed), direct-peer localStorage creds
  (gone — hub mediates).
- Driver trust model: drivers are PATH/directory-discovered binaries —
  same trust level as any installed binary; discovery order puts
  user-writable dirs after `$SEPIA_DRIVER_DIR` explicitly; log every
  discovered driver at boot.

## Upgrade / migration

Fresh state, no data migration: the Rust node mints a new node identity,
devices re-pair, config is set via env/flags. Agent stores are read-only
to us — nothing to migrate. State this in the release notes; it's a
clean cut, not an upgrade path.

## Lightweight rules

- Node daemon: axum + hyper + rusqlite + serde + tokio — that's the
  footprint. No reqwest, no sqlx, no Leptos.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`.
- `cargo bloat` + RSS smoke check in e2e — daemon idles under a stated
  budget (set during Phase 1-2, e.g. < 30 MB RSS headless).
- Keep compile graphs separable: `sepia-node` must `cargo build` without
  touching web crates.

## Phases

**Phase 0 — contracts & fixtures.** Freeze protocol v1: run TS adapters/
routes against real+fixture stores, snapshot `GET /sessions`, `history`,
`export` IR, event streams, node/config payloads into
`sepia-testkit/fixtures/`. Keep the `ACP_IT=1` gating pattern. _Risk:
fixtures go stale while TS evolves — feature-freeze the protocol surface
or make regen an `xtask` command._

**Phase 0.5 — workspace scaffolding.** Members, workspace lints, xtask,
`sepia-core` skeleton, `sepia-driver-sdk`, `sepia-testkit` shell, CI
matrix. All later work lands inside enforced structure.

**Phase 1 — driver mechanism + first driver.** `sepia-driver-sdk` (trait,
manifest, ndjson JSON-RPC loop) + `sepia-driver-host` (discovery, spawn,
supervise, registry) + `sepia-driver-devin` validated against golden
fixtures. The plugin mechanism is load-bearing — prove it first.

**Phase 2 — core + remaining drivers + convert.** `sepia-core` IR (serde),
ports, Restore/Rewind; `sepia-driver-{cline,claude,cursor}`;
`sepia-convert`. Parallelizable per driver.

**Phase 3 — ACP + control plane.** `sepia-acp` (SDK transport + tolerant
normalizer) + `sepia-control`. `ControlPlane.ts` is the riskiest backend
port — ~1700 lines of lock/idle/takeover state machine, Effect
interruption ≠ tokio cancellation. Port with its 8 test files open
side-by-side.

**Phase 4 — node + CLI.** `sepia-http` (fresh v2 REST+SSE surface — no
legacy shapes carried over) + `sepia-node` + `sepia` CLI. Gate: testkit
`assert_http_contract` + golden fixtures + Rust e2e (the React UI does
NOT serve as a harness — protocol is redesigned). Swap `bun --compile`
for cargo-dist binaries.

**Phase 5 — hub + Leptos UI + PWA.** The big one (34k LOC TSX).
Component inventory first, then by feature slice: shell + router +
token gate → session list/tree → detail + history → live chat (SSE) →
settings/nodes/transfer → `sepia-outbox` (lands with the hub — the UI
surfaces its states) → PWA layer (manifest, service worker, offline
shell = cached projection read-only + outbox view) → push (`web-push`,
VAPID keys on hub, hub subscribes to node SSE → notifies devices; iOS
caveat: requires installed PWA + iOS 16.4+).
Hub SQLite projection = indexes only.

**Phase 6 — sync + teardown.** `sepia-sync` (cursors, deltas,
tombstones). Delete the TypeScript workspace
entirely: bun/vite/vp, node_modules, lockfile, TS configs. Golden
fixtures stay (they're JSON). Port docs + release tooling.

## Final README — required "how it looks" section

The rewritten README must lead with the user-facing e2e story (draft —
polish during Phase 6, keep it truthful to what shipped):

> **Day-to-day.** Open the hub URL from any device — phone, laptop,
> anything with a browser. One token, one UI, sessions from every node
> merged into a single list. List and chat load instantly — the hub
> serves its local SQLite projection; live updates stream over SSE.
>
> **Everywhere works.** Prompt, attach, take over, cancel, restore,
> rewind — routed to the owning node automatically.
>
> **Offline is real.** A node goes down → its sessions show as offline,
> your writes **queue** (`queued`/`failed` states are visible) and drain
> in order when the node returns. The hub itself keeps serving its last
> projection read-only.
>
> **Phone.** Install the PWA — offline app shell + push notifications
> (agent finished, permission requested, session held).
>
> **Per machine.** One `sepia-node` daemon; it discovers `sepia-driver-*`
> binaries and agent CLIs on PATH at boot — an agent you don't have
> simply doesn't appear. Add an integration = drop a driver binary into
> `~/.local/share/sepia/drivers/`. Your main machine also runs
> `sepia-hub`; daemon-only machines skip it.
>
> **Ops.** `sepia pair` authorizes a device, `sepia serve` runs the
> daemon, `sepia driver list` shows what's discovered.

## Free wins to bank

- **Credentials move server-side.** Today tokens sit in `localStorage`
  (flagged deferred-insecure in TODO.md). With the hub holding node
  tokens, the browser never stores bearer credentials.
- **`bun:sqlite` constraint disappears** — everything Rust-side uses
  rusqlite; the bun-runtime requirement vanishes.
- **Mock ACP agent is first-class** — the Rust SDK builds agents too, so
  control-plane conformance tests need no JS in the loop.

## Risk ledger

1. **UI port dominates** (~60% of effort): 191 components, custom
   tree/dnd/palette/virtualization primitives with no crate equivalents.
   Mitigate: inventory + per-slice migration; React app stays alive
   against the Rust daemon until parity per slice.
2. **Rewrite + redesign coupled** — phases exist specifically to decouple
   them. Node reaches protocol parity before hub work starts.
3. **ACP SDK strictness** — covered above; `serde_json::Value` boundary.
4. **Drizzle schema drift** — devin's `sessions.db` is agent-owned; add a
   schema-version fixture test so breakage is loud.
5. **ControlPlane subtleties** — cancellation-safety bugs will be silent;
   conformance suite + replay harness for it specifically.
6. **Queued-write semantics need care.** A queued prompt landing on a
   node whose session changed state since is a real conflict: policy is
   per-op — metadata ops apply LWW, turn-shaped ops (prompt/cancel/
   permission) fail to dead-letter with UI surfacing. FIFO per session
   prevents interleaving. This is the one place "local-first" gets real.
7. **Wasm bundle size** — comrak + syntect in wasm gets heavy; SSR covers
   first paint, but lazy-load grammars/highlighting.
8. **Fixture drift** — feature-freeze protocol or regen via xtask.
9. **Thaw ≠ shadcn parity** — expect hand-built components for the
   specialized parts; don't promise pixel-parity early.
10. **No wire-compat means no cross-version interop** — old Bun nodes and
    new Rust nodes won't talk. Migration is all-at-once per node; the
    plan's sequencing (node before hub) tolerates this.
11. **Long-daemon bug class** — fd/task/memory leaks only show under
    sustained churn; the soak test exists because conformance suites
    can't see them.
12. **Browser e2e tooling is the weakest link** — `thirtyfour` is the
    honest cost of deleting all TS; if it can't hold the PWA suite, a
    minimal playwright exception beats shipping an untested UI.
