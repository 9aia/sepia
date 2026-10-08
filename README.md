# Sepia

One UI for every coding-agent session you own — Devin, Cline, Claude
Code, Cursor — across every machine you own. Written in Rust.

**Day-to-day.** Open the hub URL from any device — phone, laptop,
anything with a browser. One token, one UI, sessions from every node
merged into a single list. List and chat load instantly — the hub
serves its local SQLite projection; live updates stream over SSE.

**Everywhere works.** Prompt, attach, take over, cancel, restore,
rewind — routed to the owning node automatically.

**Offline is real.** A node goes down → its sessions show as offline,
your writes **queue** (`queued`/`failed` states are visible) and drain
in order when the node returns. The hub itself keeps serving its last
projection read-only.

**Phone.** Install the PWA — offline app shell + push notifications
(agent finished, permission requested, session held).

**Per machine.** One `sepia-node` daemon; it discovers `sepia-driver-*`
binaries and agent CLIs on PATH at boot — an agent you don't have
simply doesn't appear. Add an integration = drop a driver binary into
`~/.local/share/sepia/drivers/`. Your main machine also runs
`sepia-hub`; daemon-only machines skip it.

**Ops.** `sepia pair` authorizes a device, `sepia serve` runs the
daemon, `sepia driver list` shows what's discovered.

```
┌─ sepia-hub (Leptos SSR + sync engine) ────────────────┐
│ browser/PWA on any device — one token, one UI          │
│ SQLite projection (indexes only) + durable outbox      │
└───────┬───────────────┬───────────────┬───────────────┘
        │ HTTP+SSE      │ HTTP+SSE      │ HTTP+SSE
        ▼               ▼               ▼
   sepia-node      sepia-node      sepia-node
   (headless)      (headless)      (+ hub on main machine)
        │               │               │
        ▼               ▼               ▼
  sepia-driver-* binaries (discovered, not bundled)
        │
        ▼
  agent CLIs over ACP (devin acp, cline --acp, claude-agent-acp)
```

## Concepts

**Session IR** — the canonical representation every store reads and
writes (`sepia-core`): messages with roles and content blocks (text,
thinking + signature, tool calls, images, file attachments), tool calls
with args/results/status/diffs, token usage, sub-agent lineage,
checkpoints, and a meta overlay (title/pin/projects/spans). Run spans
record which agent on which machine produced each stretch — agent and
machine are provenance, not identity.

**Drivers** — store adapters as separate binaries
(`sepia-driver-devin`, `sepia-driver-cline`, `sepia-driver-claude`,
`sepia-driver-cursor`). Discovered at runtime from
`$SEPIA_DRIVER_DIR` → `~/.local/share/sepia/drivers/` → PATH, probed
for a manifest + capabilities (SessionStore, SessionWrite, Checkpoints,
Restore, Rewind, Convert), and driven over ndjson JSON-RPC stdio. A
dead driver respawns on next use.

**ACP** — agents attach through the Agent Client Protocol
(`sepia-acp`): spawn, initialize/capabilities, session
list/load/prompt/cancel, normalized update streams, permission
round-trips. The wire boundary is tolerant `serde_json::Value` — real
agent output is messier than any schema.

**Node authority** — each node owns its sessions' locks; a session
locked by a live process attaches read-only until a takeover. The hub
never writes a node store directly — it routes ops (or queues them
offline in `sepia-outbox`, per-session FIFO with idempotency keys).

## Layout

| Crate                                      | Role                                                                                         |
| ------------------------------------------ | -------------------------------------------------------------------------------------------- |
| `sepia-core`                               | Session IR, `SessionRepository` port, restore/rewind planners (pure)                         |
| `sepia-proto`                              | `SessionEvent` enum + node wire types                                                        |
| `sepia-driver-{sdk,host}`                  | driver serve loop / discovery + spawn + respawn                                              |
| `sepia-driver-{devin,cline,claude,cursor}` | store adapter binaries                                                                       |
| `sepia-acp`                                | tolerant ACP client over ndjson stdio                                                        |
| `sepia-control`                            | control plane: merged lists, attach/takeover/locks, prompt/cancel/permission, restore/rewind |
| `sepia-{meta,convert,outbox,push,sync}`    | overlay store, conversion, durable write queue, web-push, node→hub projection                |
| `sepia-http`                               | axum REST + SSE                                                                              |
| `sepia-node`                               | headless daemon (`sepia_node::serve`)                                                        |
| `sepia-cli`                                | `sepia` binary — store/node/config verbs, pair, service                                      |
| `sepia-{web,hub}`                          | Leptos UI + SSR host                                                                         |
| `sepia-testkit`                            | conformance suite, golden fixtures, mock ACP agent                                           |

## Build + run

```bash
cargo xtask install            # release-build sepia, sepia-node, all drivers → ~/.local/bin

# each laptop
sepia serve                    # the node daemon (API on 127.0.0.1:8787)

# main machine (also runs a node)
SEPIA_NODE_URL=http://127.0.0.1:8787 \
SEPIA_NODES='laptop=http://127.0.0.1:8787@TOKEN' \
  cargo run -p sepia-hub       # UI on 127.0.0.1:3000
```

`SEPIA_NODES` takes `id=url[@token];…` for every node the hub follows;
`SEPIA_NODE_URL`/`SEPIA_NODE_TOKEN` is the single-node shorthand. The
projection + outbox live under `$SEPIA_HOME/hub/`.

Pair a device: `sepia pair` on the node mints a code; the UI's pair
flow redeems it for a bearer token (server-side — the browser never
stores node credentials).

## Develop

```bash
cargo xtask check              # fmt + clippy + check (workspace lints are strict)
cargo xtask test               # the whole suite
cargo test -p sepia-node       # daemon e2e (real driver + mock agent + HTTP)
cargo leptos watch             # hub dev loop (wasm bundle → target/site)
```

The TypeScript implementation this replaces lives in `apps/` +
`packages/` until Phase 6 teardown (`project/plans/rust-rewrite.md`).
