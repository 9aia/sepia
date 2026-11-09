# sepia-driver-pavo — `tamias-driver-sepia`

The [pavo](https://github.com/9aia/pavo) view driver for the sepia
session plane — read-only surfaces over `sepia serve` (the ndjson
JSON-RPC service wire, `crates/sepia-cli/src/serve.rs`). Bin name is
the tamias driver contract: `tamias-driver-sepia`.

## Verbs

| verb | output |
| --- | --- |
| `manifest` | the `DriverManifest` JSON (name `sepia`, views, contributes) |
| `capabilities` | `{"capabilities": ["pavo"], "verbs": ["manifest","serve"], …}` |
| `serve` | ndjson JSON-RPC on stdio — `{"op":"manifest"|"views"|"view"|"query"}` → `{"ok":true,"result":…}` / `{"ok":false,"error":…}` (pavo-protocol v1 wire) |

Install: `cargo build --release -p sepia-driver-pavo` and put
`target/release/tamias-driver-sepia` on `PATH`, or into
`$TAMIAS_HOME/drivers/` (`~/.local/share/tamias/drivers`).

## Which `sepia` does it read?

Every `view`/`query` proxies read-side methods of one spawned
`sepia serve` subprocess per driver `serve` process — the same
framing driver binaries speak (`sepia_driver::rpc`). Only read ops
are issued (`manifest`, `agents.list`, `sessions.list`,
`sessions.summary`, `sessions.history`, `projects.list`,
`config.list`); attach/prompt/delete and friends stay on the
`sepia` CLI — the driver cannot mutate session state.

Binary resolution, first hit wins:

1. **Request params** — `sepiaBin`, `sepia`, or `bin`.
   e.g. `pavo view sepia:sessions --param sepiaBin=/path/to/sepia`.
2. **`$SEPIA_BIN`**.
3. **`sepia` next to this driver's own executable** — covers
   `cargo build` workspace layouts (`target/debug/` holds both).
4. **`sepia` on `PATH`.**

`manifest`/`views` never touch sepia state. When no binary resolves
or the spawn fails, every query degrades to an `{"error": …}`
payload inside an `ok:true` response — the wire stays healthy and
hosts render the failure inline. `serve` also warms the client in
the background so the `sepia serve` plane build overlaps the
manifest/views handshake instead of the first data op's timeout
budget; a dead serve process is respawned once per call.

## Views

| view | serve methods | params |
| --- | --- | --- |
| `sepia:status` | `manifest` + `sessions/agents/projects.list` counts | — |
| `sepia:sessions` | `sessions.list` | `agent` |
| `sepia:session` | `sessions.list` (match) → `sessions.summary` fallback + `sessions.history` | `id` (required), `agent`, `limit` |
| `sepia:agents` | `agents.list` + `config.list` | `agent`, `dir` |
| `sepia:projects` | `projects.list` + `sessions.list` | — |

`sessions.list` is called **without** `withLocks`: lock probing fans
a `session/list` probe out to every registered agent and a slow
store driver can hold the call past a host's request timeout.
`busy` (live attach state) and the overlay flags (pinned/archived)
still come through; `locked` therefore reads false.
