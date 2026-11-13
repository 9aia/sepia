# Use sepia in pavo

`tamias-driver-sepia` is sepia's pavo surface: a view driver that
proxies the `sepia serve` session plane plus an ops manifest that
exposes every bounded `sepia` CLI verb. It ships inside the sepia
tamias pack (`libraries/sepia`) — `tamias install` places the driver
binary where pavo discovers it (`PATH` or `$TAMIAS_HOME/drivers`),
so no separate install step exists.

## Open a view

```bash
pavo open sepia:live                  # TUI — live sessions + feed health
pavo open sepia:permissions           # pending permission asks
pavo view sepia:sessions --format json   # CLI render, no TUI
pavo views --driver sepia             # every <driver>:<view> ref
```

| View | Route | Shows |
| ---- | ----- | ----- |
| `status` | `/sepia/status` | Plane digest — node identity, protocol, session/agent counts |
| `sessions` | `/sepia/sessions` | `sessions.list` rows with busy/overlay flags |
| `session` | `/sepia/session` | One session's detail + meta-edit form + paged history; needs `--param id=` |
| `agents` | `/sepia/agents` | Runnable ACP agents + config-IR item counts |
| `config` | `/sepia/config` | Per-agent config inventory + selected agent's config IR tree; `--param agent=`/`dir=` |
| `stores` | `/sepia/stores` | `store list` per agent store, merged (cli lane) |
| `drivers` | `/sepia/drivers` | Discovered `sepia-driver-*` binaries (`driver list --json`) |
| `live` | `/sepia/live` | Feed-buffered sessions — busy/attached/detached, last event, pending asks |
| `stream` | `/sepia/stream` | One session's transcript — history head + live event tail; needs `--param id=` |
| `permissions` | `/sepia/permissions` | Pending `session/request_permission` asks |

Every view declares `refresh_ms` — the host's auto-refresh hint
(2s for the feed-buffer views, up to 30s for discovery/config). The
CLI ignores it; the TUI and web host poll on it.

## Actions and forms

`pavo actions sepia:<view>` lists what a view offers; `pavo invoke`
dispatches. Two lanes:

- `session.<verb>` ids (`attach`, `detach`, `prompt`, `cancel`,
  `permission`, `set_mode`, `set_permission_policy`, `patch_meta`,
  `export`, `restore`, `rewind`, `delete`, `create`, `import`,
  `convert`, `list`, `get`, `summary`, `history`, `prompt_wait`) are
  the curated session-plane actions — they ride the `sepia serve`
  wire, so a mutation sees exactly the plane an orchestrator would.
- Bare verb names (`store.import`, `driver.list`, `config.diff`, …,
  `cli.` prefix tolerated) ride the engine's own argv — every
  bounded `sepia` CLI verb is invocable, derived from the clap tree.
  `serve` is a surface, never an op.

```bash
pavo invoke sepia:sessions session.prompt \
  --param id=<session> --param text="hello" --yes
pavo invoke sepia:stores store.export \
  --param session_id=<id> --param from=devin --param format=json
pavo submit sepia:session --form meta \
  --param title="Renamed" --param pinned=true
```

Danger levels ride the action: `delete`/`restore`/`rewind` are
`type-target` (the id *is* the confirmation), most mutations
`confirm` (`--yes` off-TTY), reads `none`. The `session` view's meta
form submits `session.patch_meta`; field values seed from the
session's current detail.

## The live feed and permissions inbox

At `serve` start the driver warms a second `sepia serve` child
subscribed to `events.subscribe` — a pure notification stream the
request/response client can't share. Its buffer backs the `live`,
`stream`, and `permissions` queries plus two platform feeds:

- `{"op":"jobs"}` — sessions mid-turn (`runStarted` without
  `runFinished`), routed to `/sepia/stream?id=<id>`.
- `{"op":"notifications"}` — pending permission asks (warning,
  routed to `/sepia/permissions`) plus feed-health alerts
  (`down`/`lagged`).

On the CLI: `pavo jobs` / `pavo notifications` aggregate every
provider's feed.

A pending ask clears on the first of: a successful
`sessions.permission` (the `session.permission` action — the
`permissions` row supplies `id` + `request`, pick `option` from the
row's options column), `runFinished` for the session, or
detach/delete. Asks older than an hour hide as a wedged-feed
failsafe. `feed.status` (the kv atop `live`/`permissions`) reports
`starting`/`live`/`stale`/`down`, failure count, buffered session and
pending-ask counts, cumulative `lagged` drops, and recent lifecycle
markers; a `down` feed re-arms lazily on the next feed query.

## Steering params

`--param` keys the driver understands:

- `sepiaBin` (aliases `sepia`, `bin`) — path to a `sepia`-shaped
  executable for the *request* lane (view queries + `session.*`
  actions). Resolution order: param → `$SEPIA_BIN` → `sepia` next to
  the driver binary → the driver itself (`run serve`) → `sepia` on
  `PATH`. The feed child resolves independently (env/sibling/self —
  `sepiaBin` does not steer it).
- `id` — session id for `session`/`stream`/`history`-bound views and
  session-scoped actions; `agent` disambiguates ids that collide
  across stores.
- `agent`, `dir` — forwarded to `agents.list`/`config.*` calls.
- Store flags on `stores`/store verbs: `db`, `data-dir`,
  `claude-dir`, `cursor-dir`.
- `$SEPIA_DRIVER_DIR` (env, inherited — not a param) — the spawned
  `sepia serve` plane resolves its `sepia-driver-*` store drivers
  `$SEPIA_DRIVER_DIR` → `<data dir>/sepia/drivers` →
  `/usr/local/lib/sepia/drivers` → `PATH`; the driver's `drivers`
  view reports whatever that scan finds.

## Degraded mode

Reads never fail the wire: an unreachable `sepia serve` (or a dead
feed child) degrades to `{"error": …}` payloads inside `ok:true`
responses — views render the failure inline, `feed.status` reports
`stale`/`down` with the cause, and `cli.*` ops report the engine's
own nonzero exit as honest `{code, stdout, stderr}` data. The data
client respawns once per call on transport death; the feed
supervisor backs off 1s→5s and gives up after five consecutive
subscribe failures until the next query re-arms it.
