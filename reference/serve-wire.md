# The `sepia serve` wire

`sepia serve` exposes the local session plane as ndjson JSON-RPC over
stdio — the same framing `sepia-driver-*` binaries speak, described in
[the driver protocol](protocol.md). One JSON object per line, requests
and responses interleaved; `session.event` notifications arrive
unsolicited after `sessions.subscribe`.

## Handshake

```json
{"jsonrpc":"2.0","id":1,"method":"manifest","params":{}}
```

The manifest reports `name`, `version`, `protocol` (the wire version —
bumped on breaking changes), and `methods`. Orchestrators should check
`protocol` before issuing calls; peregrin's `peregrin-session` crate
refuses to bind a mismatched service.

## Methods

| Method | Plane call |
| --- | --- |
| `manifest` | Service manifest + method list |
| `agents.list` | Registered agents + capabilities |
| `sessions.list` | `list_sessions` (`withLocks` param probes live lock state) |
| `sessions.get` | Full session IR (`id`, optional `agent`) |
| `sessions.summary` | Metadata-only session |
| `sessions.history` | Paginated history (`id`, `limit`, `before`) |
| `sessions.attach` | Attach (`id`, `takeover`, `model`, `fallbacks`) |
| `sessions.detach` | Release a held session |
| `sessions.create` | New session (`cwd`, `agent`, `title`, `model`, `fallbacks`) |
| `sessions.prompt` | Send a turn (`id`, `parts`, `agent`) |
| `sessions.cancel` | Cancel a running turn |
| `sessions.permission` | Resolve a permission request (`id`, `request`, `option`) |
| `sessions.delete` | Delete from the owning store |
| `sessions.restore` | Restore rewindable files (`id`, `request`) |
| `sessions.rewind` | Rewind messages/history (`id`, `request`) |
| `sessions.subscribe` | Subscribe → `session.event` notifications |
| `shutdown` | Close the plane, exit 0 |

## Errors

Failures come back as JSON-RPC errors with `data.code` matching the
plane's `ControlErrorCode` (`not_found`, `locked`, `invalid_params`,
`internal`, …) and `data.cause` carrying the underlying store/driver
message. A spawner that respawns the process should treat a dead child
as retryable for idempotent reads; peregrin respawns once per call.

## Discovery and environment

Driver discovery and all `SEPIA_*` env vars apply to the `sepia serve`
child, not the spawner — set them in its environment (`SEPIA_DRIVER_DIR`,
`SEPIA_CLAUDE_DIR`, `SEPIA_CLINE_DIR`, `SEPIA_DEVIN_DB`, lock TTLs).
The authoritative list is `crates/sepia-host` + each driver's binary
source.
