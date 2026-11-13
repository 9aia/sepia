# sepia-driver-pavo — `tamias-driver-sepia`

The [pavo](https://github.com/9aia/pavo) view driver for the sepia
session plane — views proxying `sepia serve` (the ndjson JSON-RPC
service wire, `crates/sepia-cli/src/serve.rs`) plus an engine ops
surface exposing every bounded `sepia` CLI verb. Bin name is the
tamias driver contract: `tamias-driver-sepia`.

## Verbs

| verb | output |
| --- | --- |
| `manifest` | the `DriverManifest` JSON (name `sepia`, views, contributes) |
| `capabilities` | `{"capabilities": ["pavo"], "verbs": ["manifest","serve"], …}` |
| `ops` | the engine ops manifest — every bounded `sepia` verb as a `cli.<verb>` query, `run`/`cli` passthroughs, session-plane actions |
| `run <argv>` | engine passthrough — `tamias-driver-sepia run list` ≡ `sepia list` |
| `cli <argv>` | the tamias `bins[]` shim verb — strips `--dir`/`--name`, runs the engine in-process with inherited stdio (the `sepia` command itself) |
| `serve` | ndjson JSON-RPC on stdio — `{"op":"manifest"|"views"|"view"|"query"|"invoke"|"submit"|"jobs"|"notifications"}` → `{"ok":true,"result":…}` / `{"ok":false,"error":…}` (pavo-protocol v1 wire) |
| `mcp` | *(feature `mcp`)* the ops manifest as MCP tools on stdio |

There is no hand-rolled `sepia` binary: the command ships as the
tamias pack in `libraries/sepia/` — `cargo xtask pack` stages this
driver into `drivers/bin/<triple>/` and `tamias install
--allow-drivers ./libraries/sepia` deploys the generated `sepia`
shim → `cli` verb → `sepia_cli::run_from` (T-012, the cuora P-023
pilot replicated).

Install: `cargo build --release -p sepia-driver-pavo` and put
`target/release/tamias-driver-sepia` on `PATH`, or into
`$TAMIAS_HOME/drivers/` (`~/.local/share/tamias/drivers`).

## Which `sepia` does it read?

Every `view`/`query` proxies read-side methods of one spawned
`sepia serve` subprocess per driver `serve` process — the same
framing driver binaries speak (`sepia_driver::rpc`). View queries
stay read-side (`manifest`, `agents.list`, `sessions.list`,
`sessions.summary`, `sessions.history`, `config.list`); mutations
go through the `invoke`/`submit` interaction ops, which dispatch
`session.*` actions over the same serve wire and `cli.*` actions
through the engine argv path — a mutation sees exactly the plane an
orchestrator would.

Binary resolution, first hit wins:

1. **Request params** — `sepiaBin`, `sepia`, or `bin`.
   e.g. `pavo view sepia:sessions --param sepiaBin=/path/to/sepia`.
2. **`$SEPIA_BIN`**.
3. **`sepia` next to this driver's own executable** — covers
   `cargo build` workspace layouts (`target/debug/` holds both).
4. **This driver itself** — `<self> run serve`: the `sepia` binary
   is retired (it is a tamias `bins[]` shim now), so the engine's
   `run` passthrough is the same entrypoint with no PATH hop.
5. **`sepia` on `PATH`** — last resort when `current_exe` fails.

`manifest`/`views` never touch sepia state. When no binary resolves
or the spawn fails, every query degrades to an `{"error": …}`
payload inside an `ok:true` response — the wire stays healthy and
hosts render the failure inline. `serve` also warms the client in
the background so the `sepia serve` plane build overlaps the
manifest/views handshake instead of the first data op's timeout
budget; a dead serve process is respawned once per call.

## The live feed

`serve` warms a second `sepia serve` child subscribed to
`events.subscribe` — a pure `node.event` notification stream the
request/response client can't share. Its bounded buffer backs the
`live`/`stream`/`permissions` queries plus the `jobs`/
`notifications` platform feeds; `feed.status` reports health
(`starting`/`live`/`stale`/`down`). Pending `session/request_permission`
asks clear on a successful `sessions.permission`, `runFinished`,
or detach/delete. The feed child resolves its binary independently
of `--param sepiaBin=` (env/sibling/self only).

## Views

| view | route | serve methods / lane | params |
| --- | --- | --- | --- |
| `sepia:status` | `/sepia/status` | `manifest` + `sessions/agents.list` counts | — |
| `sepia:sessions` | `/sepia/sessions` | `sessions.list` | `agent` |
| `sepia:session` | `/sepia/session` | `sessions.list` (match) → `sessions.summary` fallback + `sessions.history` + meta form | `id` (required), `agent`, `limit` |
| `sepia:agents` | `/sepia/agents` | `agents.list` + `config.list` | `agent`, `dir` |
| `sepia:config` | `/sepia/config` | `config.list` + `config.get` | `agent`, `dir` |
| `sepia:stores` | `/sepia/stores` | `store list` per agent (cli lane) | `db`, `data-dir`, `claude-dir`, `cursor-dir` |
| `sepia:drivers` | `/sepia/drivers` | `driver list --json` (cli lane) | — |
| `sepia:live` | `/sepia/live` | feed buffer + `sessions.list` + `feed.status` | — |
| `sepia:stream` | `/sepia/stream` | `sessions.history` head + feed tail | `id` (required), `agent` |
| `sepia:permissions` | `/sepia/permissions` | feed buffer + `feed.status` | — |

Every view declares `refresh_ms` (2s feed-buffer views → 30s
discovery/config) — the host's auto-refresh hint; the CLI ignores
it. `sessions.list` is called **without** `withLocks`: lock probing
fans a `session/list` probe out to every registered agent and a slow
store driver can hold the call past a host's request timeout.
`busy` (live attach state) and the overlay flags (pinned/archived)
still come through; `locked` therefore reads false.
