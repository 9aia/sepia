# The `sepia serve` wire

`sepia serve` exposes the local session plane as ndjson JSON-RPC over
stdio — the same framing `sepia-driver-*` binaries speak, described in
[the driver protocol](protocol.md). One JSON object per line, requests
and responses interleaved; `session.event` notifications arrive
unsolicited after `sessions.subscribe`, `node.event` notifications after
`events.subscribe`.

## Handshake

```json
{"jsonrpc":"2.0","id":1,"method":"manifest","params":{}}
```

The manifest reports `name`, `version`, `protocol` (the wire version —
bumped on breaking changes), `node` (`{id, name}` — the machine identity
minted at `$SEPIA_HOME/node.json`), `config`, and `methods`.
Orchestrators should check `protocol` before issuing calls; peregrin's
`peregrin-session` crate refuses to bind a mismatched service.

`config` is the plane's resolved lifecycle tuning — what the `SEPIA_*`
env and defaults actually produced, so a spawner can verify its env took
effect on the child:

```json
{"idleTtlMs":600000,"sweepMs":30000,"historyLimit":500,"probeIdleMs":60000}
```

`idleTtlMs` is `null` when the idle sweep is disabled (a zero TTL);
`probeIdleMs` is the resolved lock-probe pool TTL — the `probe_idle`
override, or ten lock TTLs floored at 60s.

## Methods

| Method | Plane call |
| --- | --- |
| `manifest` | Service manifest + method list |
| `agents.list` | Registered agents + capabilities |
| `sessions.list` | `list_sessions` (`withLocks` param probes live lock state) |
| `sessions.get` | Full session IR (`id`, optional `agent`) |
| `sessions.summary` | Metadata-only session |
| `sessions.export` | Full session IR wrapped as `{session}` — the transfer bundle (`id`, `agent?`) |
| `sessions.import` | Write IR into an agent's store via `session.save` (`agent`, `session`, `id?`, `cwd?`, `title?`, `model?`) → `{id, agentId}`; mints a fresh id unless `id` is passed |
| `sessions.convert` | Node-local copy of a stored session into another agent's store (`id`, `agent`, `from?`) → `{sessionId, agentId}` |
| `sessions.history` | Paginated history (`id`, `limit`, `before`) |
| `sessions.attach` | Attach (`id`, `takeover`, `model`, `fallbacks`) |
| `sessions.detach` | Release a held session |
| `sessions.create` | New session (`cwd`, `agent`, `title`, `model`, `fallbacks`) |
| `sessions.prompt` | Send a turn (`id`, `parts`, `agent`) |
| `sessions.cancel` | Cancel a running turn |
| `sessions.permission` | Resolve a permission request (`id`, `request`, `option`) |
| `sessions.delete` | Delete from the owning store |
| `sessions.patch_meta` | Meta overlay patch (`id`, `patch`: `title`/`pinned`/`archived`/`projectIds`/`model`) |
| `projects.list` | Node-local projects |
| `projects.create` | New project (`name`) |
| `projects.rename` | Rename (`id`, `name`) |
| `projects.delete` | Delete (`id`) — dropped from every session's `projectIds` |
| `config.list` | Per-agent config inventory (`agent?`, `dir?`) → `[{agent, counts}]` for every `AgentConfig`-capable driver; a failing driver yields `{agent, error}` |
| `config.get` | Agent-config IR (`agent`, `dir?`) via the driver's `config.read` — the serialized `AgentConfig` (per-item + config `metadata` included) |
| `config.import` | Install a config IR payload (`agent`, `dir?`, `config`) via `config.write` → `{actions}` — `config` accepts the `config export` JSON or a serialized `AgentConfig` |
| `sessions.restore` | Restore rewindable files (`id`, `request`) |
| `sessions.rewind` | Rewind messages/history (`id`, `request`) |
| `sessions.subscribe` | Subscribe (`id`) → `session.event` notifications |
| `events.subscribe` | Node-wide feed → `node.event` notifications |
| `shutdown` | Close the plane, exit 0 |

## Event streams

`sessions.subscribe` returns `{subscribed, sessionId}` and forwards the
session's translated frames as `session.event` notifications —
`{sessionId, events: [SessionEvent…]}`. A repeat subscribe on the same
session spawns a second forwarder: notifications duplicate, they are not
deduped. If the receiver falls behind the 512-event buffer, a
`{sessionId, events: [], lagged: n}` marker reports the dropped-batch
count.

`events.subscribe` returns `{subscribed, live}` — `live` is the list of
session ids attached at subscribe time — and streams `node.event`
notifications carrying lifecycle markers plus every live session's
events. One subscription covers the whole node; a racing attach can
appear in both `live` and the stream, so dedup by `sessionId`. The
`type` field discriminates:

| `type` | Payload |
| --- | --- |
| `attached` | `{sessionId, agentId}` — an existing store session went live |
| `created` | `{sessionId, agentId, cwd}` — a fresh session was created live |
| `detached` | `{sessionId, agentId, reason}` — left the live map; `reason` is `explicit`, `idle` (the sweep retired it), `deleted`, `rewind`, or `shutdown`. Always the session's last event on the feed |
| `deleted` | `{sessionId, agentId}` — the store row was deleted through its agent |
| `sessionEvents` | `{sessionId, events: [SessionEvent…]}` — the same batches `sessions.subscribe` forwards; `runStarted`/`runFinished` mark the busy transitions |
| `lagged` | `{sessionId, count}` — `sessionId` `null` means the node feed overflowed for this receiver |

Markers mirror live-map membership exactly: a read-only attach or a
failed load emits nothing.

## Errors

Failures come back as JSON-RPC errors with `data.code` matching the
plane's `ControlErrorCode` (`not_found`, `locked`, `invalid_params`,
`internal`, …) and `data.cause` carrying the underlying store/driver
message. `sessions.import`/`sessions.convert` answer `invalid` on a
malformed `session` payload and `unknown_agent` when no `SessionWrite`
driver exists for `agent`. `config.get`/`config.import`/`config.list`
answer `unknown_agent` when `agent` is absent or its driver lacks the
`AgentConfig` capability; `config.import` answers `invalid` on a
malformed `config` payload. `dir` is the config root the driver's
adapter expects — `.claude`/`.cursor`/devin config dirs, or a Cline
*workspace root*; absent, the driver resolves its own default
(`SEPIA_*_DIR` envs, user dir, or the driver's cwd for Cline). Meta
verbs (`sessions.patch_meta`, `projects.*`) answer
`invalid` on a plane built without a meta store. A spawner that
respawns the process should treat a dead child as retryable for
idempotent reads; peregrin respawns once per call.

Session rows in `sessions.list` carry the meta overlay merged in
(`pinned`, `archived`, `projectIds`, `spans`, title override) plus
pending rows for sessions only the overlay knows about — created
through `sessions.create` but never flushed to the agent's store.
`sessions.patch_meta`'s `model` field takes `null` to clear the stored
spawn preference (applied on the next `sessions.attach`).

## Discovery and environment

Driver discovery and all `SEPIA_*` env vars apply to the `sepia serve`
child, not the spawner — set them in its environment (`SEPIA_DRIVER_DIR`,
`SEPIA_CLAUDE_DIR`, `SEPIA_CLINE_DIR`, `SEPIA_DEVIN_DB`, lock TTLs).
The authoritative list is `crates/sepia-host` + each driver's binary
source.
