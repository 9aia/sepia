# The driver wire

Every `sepia-driver-*` binary speaks ndjson JSON-RPC 2.0 over
stdin/stdout — one JSON object per line, requests and responses
interleaved. The driver binary is the **server**; `sepia-host` (inside
`sepia serve` or the `sepia` CLI) is the **client**. `sepia serve`
itself answers the same framing on its own method surface — see
[serve-wire](serve-wire.md).

```json
{"jsonrpc":"2.0","id":1,"method":"driver.manifest","params":{}}
{"jsonrpc":"2.0","id":1,"result":{"id":"cline","version":"0.0.0","protocol":1,"capabilities":["sessionStore","sessionWrite","agentConfig"],"agentCommand":"cline --acp","configSchema":{}}}
```

Params and results are camelCase JSON. Notifications have no `id`;
drivers currently emit none — every outbound line answers an inbound
request or is a protocol error.

## Manifest

`driver.manifest` is the handshake every host issues first:

| field | meaning |
| --- | --- |
| `id` | driver id — `cline`, `claude`, `cursor`, `devin`, … |
| `version` | driver build version |
| `protocol` | driver-wire version; the host refuses a mismatch |
| `capabilities` | which method families the driver serves (below) |
| `agentCommand` | the agent CLI argv the host resolves on PATH — an agent only appears when the driver *and* its command resolve |
| `configSchema` | JSON Schema for driver-specific config |

| Capability | Methods it unlocks |
| --- | --- |
| `sessionStore` | `session.list`/`get`/`summary`/`history`/`checkpoints` — reads |
| `sessionWrite` | `session.save`/`rename`/`delete` — writes; `save` is the *install* path (the driver writes its full store layout, e.g. cline's manifest + transcript + index row) |
| `checkpoints` | `session.checkpoints` — workspace snapshot refs |
| `restore` | `file.restore` — file restore from recorded diffs/checkpoints |
| `rewind` | `session.rewind` — transcript truncation |
| `convert` | cross-store conversion between two backends |
| `agentConfig` | `config.read`/`config.write` — the agent-config IR |

## Methods

| Method | Params → Result |
| --- | --- |
| `session.list` | `{}` → session summaries |
| `session.get` | `{id, agentId?}` → full session IR |
| `session.summary` | `{id, agentId?}` → metadata-only session |
| `session.history` | `{id, limit?, before?, agentId?}` → a message page |
| `session.save` | `{session}` → `{}` — write the IR into the store (the import/install path) |
| `session.rename` | `{id, title}` |
| `session.delete` | `{id}` |
| `session.checkpoints` | `{id}` → snapshot refs |
| `session.rewind` | `{session, plan, truncated}` — the plane ships the full pre-rewind IR, the computed cut, and the already-truncated IR; the driver picks its store mechanism |
| `file.restore` | `{id, path, toolCallId?, confirm}` — `confirm` is the mandatory destructive-op gate → `{restored, skipped:[{path,reason}]}` |
| `config.read` | `{dir?}` → `AgentConfig` IR — `dir` is the config root the driver's adapter expects (a `.claude`/`.cursor`/devin dir, or a Cline *workspace root*); absent, the driver resolves its default (`SEPIA_*_DIR`, user dir, or its own cwd) |
| `config.write` | `{dir?, config}` → `{actions:[{path,action,detail?}]}` — `config` takes the `config export` payload or a serialized `AgentConfig`; items the format can't express come back `skipped` honestly |

Session ids are the agent store's own; `agentId` params disambiguate
colliding ids across backends in multi-store compositions.

## Errors

Standard JSON-RPC codes plus sepia's application range (`-320xx`):

| code | name | meaning |
| --- | --- | --- |
| `-32700` | parse error | malformed line |
| `-32601` | method not found | unknown or un-capable method |
| `-32602` | invalid params | bad shape / missing `confirm` |
| `-32001` | session not found | unknown `id` |
| `-32002` | session busy | a turn is in flight |
| `-32003` | session locked | held by a live agent process |
| `-32004` | capability unsupported | method needs a capability the driver lacks |
| `-32005` | store error | backend I/O failure — `message` carries the cause |

## Discovery and lifecycle

`sepia-host` finds drivers in order: `$SEPIA_DRIVER_DIR` →
`~/.local/share/sepia/drivers` → `/usr/local/lib/sepia/drivers` → PATH
scan for `sepia-driver-*`. A `<name>.driver.toml` sibling can supply
the manifest without spawning; otherwise the host spawns, handshakes,
and supervises the process (kill-on-drop, stderr ring buffer). Drivers
are spawned per call-site as needed and reused; a dead child is
respawned on the next call.

Store dir envs per driver (`SEPIA_CLINE_DIR`, `SEPIA_CLAUDE_DIR`,
`SEPIA_CURSOR_DIR`, `SEPIA_DEVIN_DB`, `SEPIA_DEVIN_DIR`) resolve inside
the *driver* process — set them in the environment of whatever spawns
it.

## Conventions for new drivers

- Blocking I/O (SQLite, file walks) runs on `spawn_blocking`.
- Items a store can't express report `skipped`/degraded in the action
  list rather than failing or silently dropping — loss is reported,
  never hidden.
- `sepia-testkit` contract suites exercise the invariants every driver
  must satisfy; golden fixtures freeze the wire shape.
