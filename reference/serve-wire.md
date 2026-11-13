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
| `sessions.attach` | Attach (`id`, `takeover`, `model`, `fallbacks`, `permissions`) |
| `sessions.detach` | Release a held session |
| `sessions.create` | New session (`cwd`, `agent`, `title`, `model`, `fallbacks`, `permissions`) |
| `sessions.prompt` | Send a turn (`id`, `parts`, `agent`) — resolves when the agent finishes the turn, returns `null` |
| `sessions.prompt_wait` | Prompt + await the turn's terminal state (`id`, `parts`, `agent?`, `timeoutMs?`) → `PromptWaitResult` |
| `sessions.cancel` | Cancel a running turn |
| `sessions.permission` | Resolve a permission request (`id`, `request`, `option`) |
| `sessions.set_mode` | Switch the agent mode (`id`, `mode`, `agent?`) — `session/set_mode` |
| `sessions.set_permission_policy` | Set the permission policy (`id`, `permissions`, `agent?`) |
| `sessions.delete` | Delete from the owning store |
| `sessions.patch_meta` | Meta overlay patch (`id`, `patch`: `title`/`pinned`/`archived`/`projectIds`/`model`) |
| `config.list` | Per-agent config inventory (`agent?`, `dir?`) → `[{agent, counts}]` for every `AgentConfig`-capable driver; a failing driver yields `{agent, error}` |
| `config.get` | Agent-config IR (`agent`, `dir?`) via the driver's `config.read` — the serialized `AgentConfig` (per-item + config `metadata` included) |
| `config.import` | Install a config IR payload (`agent`, `dir?`, `config`) via `config.write` → `{actions}` — `config` accepts the `config export` JSON or a serialized `AgentConfig` |
| `sessions.restore` | Restore rewindable files (`id`, `request`) |
| `sessions.rewind` | Rewind messages/history (`id`, `request`) |
| `sessions.subscribe` | Subscribe (`id`, `keepAlive?` — `false` = non-pinning watch) → `session.event` notifications |
| `events.subscribe` | Node-wide feed → `node.event` notifications |
| `shutdown` | Close the plane, exit 0 |

## `sessions.prompt_wait`

The blocking counterpart of `sessions.prompt` — the primitive an
orchestrator uses to drive an agent to completion without subscribing:

```json
{"jsonrpc":"2.0","id":4,"method":"sessions.prompt_wait","params":{"id":"<session>","parts":[{"type":"text","text":"fix the build"}],"timeoutMs":300000}}
```

Params mirror `sessions.prompt` plus `timeoutMs` (milliseconds; `0` or
absent waits until the turn settles, bounded only by agent liveness).
The result is the settled-turn report:

```json
{"sessionId":"…","agentId":"devin","stopReason":"end_turn","text":"…","eventCount":12,"laggedEvents":0,"durationMs":8421.3,"timedOut":false}
```

- `stopReason` — the agent's terminal reason (`end_turn`, `cancelled`,
  `max_tokens`, `refusal`, …); absent when the turn errored before the
  agent answered or the wait timed out mid-flight.
- `text` — the assistant text observed during the turn (concatenated
  `textMessageContent` deltas); `laggedEvents` counts broadcast batches
  the waiter dropped, so a nonzero value means `text` may be partial.
- `timedOut` — the timeout elapsed before the turn settled. The wait
  sends `session/cancel` and gives the turn a short settle window; a
  turn that outlives it keeps running in the background (its events
  still stream to subscribers) and `stopReason` stays absent.
- `error` — present on turn-level failure (agent died mid-turn, ACP
  error); pre-dispatch rejections (`invalid`, `busy`, …) still come
  back as JSON-RPC errors.

Requests dispatch concurrently — a `prompt_wait` (or a long
`sessions.prompt`) in flight does not starve `sessions.cancel`,
`sessions.permission`, or the subscribe verbs. Responses are
id-correlated; completion order is not guaranteed.

## Permission policy

Agents gate tool use behind ACP `session/request_permission` — an
unattended `prompt_wait` deadlocks when nothing answers. Each live
session carries a `PermissionPolicy`, default `ask` (the request
broadcasts as an `acp:permission_request` custom event and waits for
`sessions.permission`). Set it at create/attach time with the
`permissions` param, or switch it live with
`sessions.set_permission_policy`; both accept:

- `"ask"` — broadcast only, manual answering (the default).
- `"allow_all"` — auto-select the best allow option (`allow_once`
  preferred, then `allow_always`, then any `allow*` kind).
- `"deny_all"` — auto-select the best reject option (`reject_once`,
  `reject_always`, `reject*`), else the cancelled outcome.
- `{"rules": [{"kind": "execute", "allow": false}, …]}` — first
  matching rule decides on the tool-call `kind` (`execute`, `edit`,
  `read`, …; a rule without `kind` matches everything); unmatched
  requests fall back to `ask`.

```json
{"jsonrpc":"2.0","id":5,"method":"sessions.set_permission_policy","params":{"id":"<session>","permissions":{"rules":[{"kind":"execute","allow":false},{"allow":true}]}}}
```

The request event still broadcasts under every policy — subscribers
observe asks the policy auto-settled. With no matching option kind the
plane sends the cancelled outcome rather than guessing an option.
`create`/`attach` echo the applied policy as `permissions` in their
result (`"ask"` when none was given or the attach was read-only).

## Event streams

`sessions.subscribe` returns `{subscribed, sessionId}` and forwards the
session's translated frames as `session.event` notifications —
`{sessionId, events: [SessionEvent…]}`. A repeat subscribe on the same
session spawns a second forwarder: notifications duplicate, they are not
deduped. If the receiver falls behind the 512-event buffer, a
`{sessionId, events: [], lagged: n}` marker reports the dropped-batch
count.

A subscriber counts as a *listener* for idle tracking: while one is
attached the session never idle-detaches, so its agent subprocess stays
alive. Pass `"keepAlive": false` for the non-pinning watch — the same
`session.event` stream, but the session still idle-detaches on
`idleTtlMs` and the stream closes with it. Background consumers
(orchestrator busy-tracking, log mirrors, one-shot `prompt_wait`-style
callers) should use it; an interactive viewer that wants the session
kept warm keeps the default.

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
message. `sessions.import` answers `invalid` on a malformed `session`
payload; `sessions.convert` takes `{id, agent, from?}` — it reads the
session from the store by id (`not_found` when absent) and answers
`unknown_agent` when no `SessionWrite` driver exists for `agent`.
`config.get`/`config.import`/`config.list` answer `unknown_agent` when
`agent` is absent or its driver lacks the
`AgentConfig` capability; `config.import` answers `invalid` on a
malformed `config` payload. `dir` is the config root the driver's
adapter expects — `.claude`/`.cursor`/devin config dirs, or a Cline
*workspace root*; absent, the driver resolves its own default
(`SEPIA_*_DIR` envs, user dir, or the driver's cwd for Cline). The
meta verb (`sessions.patch_meta`) answers `invalid` on a plane built
without a meta store. A spawner that
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
