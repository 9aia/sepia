# Deploying sepia (Rust)

Sepia is a self-hosted control plane for coding-agent sessions. Each
machine runs a headless **`sepia-node`** daemon; one designated machine
also runs **`sepia-hub`** (Leptos SSR + the sync engine). Browsers —
including the phone PWA — only ever talk to the hub.

```
phone / any laptop browser ──► sepia-hub (:3000)
                                   │ HTTP+SSE
            ┌──────────────────────┼───────────────────────┐
            ▼                      ▼                       ▼
       sepia-node (:8787)     sepia-node              sepia-node
            │                      │                       │
        sepia-driver-* binaries (discovered, not bundled)
            │
        agent CLIs over ACP (devin acp, cline --acp, claude-agent-acp)
```

## Install

```bash
cargo xtask install
# release-builds and copies to ~/.local/bin:
#   sepia, sepia-node, sepia-hub,
#   sepia-driver-{devin,cline,claude,cursor}
```

Install only what the machine needs — a headless laptop wants
`sepia-node` + its driver binaries; the UI host wants `sepia-hub` too.
Drivers can also live in `$SEPIA_DRIVER_DIR`,
`~/.local/share/sepia/drivers/`, or anywhere on `PATH` — the node scans
all three at boot.

## `sepia-node`

```bash
sepia serve            # or: sepia-node
```

Key env (full list in `crates/sepia-http/src/env.rs`):

- `SEPIA_HOST` (default `127.0.0.1`), `SEPIA_PORT` (default `8787`).
  Non-loopback binds require `SEPIA_TOKEN`.
- `SEPIA_TOKEN` — bearer auth on `/api/*` (except `GET /api/health`,
  `POST /api/pair`). `?access_token` authenticates the two SSE GETs.
- `SEPIA_DRIVER_DIR` — extra driver scan dir.
- `SEPIA_DEVIN_DB` / `SEPIA_CLINE_DIR` / `SEPIA_CLAUDE_DIR` /
  `SEPIA_CURSOR_DIR` — store roots (devin `sessions.db`, Cline data dir,
  `~/.claude`, `~/.cursor`).
- `SEPIA_HOME` (default `~/.local/share/sepia`) — meta.json, node
  identity, driver dir.
- `SEPIA_ORIGINS`, `SEPIA_AGENT_<ID>_COMMAND` (override an agent's
  spawn command), `SEPIA_IDLE_TTL_MS`, `SEPIA_SWEEP_MS`,
  `SEPIA_LOCK_TTL_MS`, `SEPIA_HELD_WATCH_MS`, `SEPIA_HISTORY_LIMIT`,
  `SEPIA_SSE_KEEPALIVE_MS`, `SEPIA_INHERIT_ENV`, `SEPIA_DEBUG`.

A node is headless — no UI deps, idle RSS in the tens of MB. Drivers
that crash respawn on next use; a machine with no Devin CLI just shows
no Devin sessions.

## `sepia-hub`

```bash
SEPIA_NODES='laptop=http://10.0.0.2:8787@TOKEN;tower=http://10.0.0.3:8787' \
SEPIA_HUB_PORT=3000 \
  sepia-hub
```

- `SEPIA_NODES` — `id=url[@token];…` for every node the hub follows.
  `SEPIA_NODE_URL` / `SEPIA_NODE_TOKEN` is the single-node shorthand.
- `SEPIA_HUB_HOST`/`SEPIA_HUB_PORT` (defaults `127.0.0.1:3000`;
  `SEPIA_PORT` also accepted). A non-loopback `SEPIA_HUB_HOST`
  (`0.0.0.0`, `::`, a LAN IP) refuses to boot without `SEPIA_HUB_TOKEN`.
- `SEPIA_HUB_TOKEN` — browser→hub auth on every non-asset route.
  Accepted as `Authorization: Bearer`, `?token=`, or the httpOnly
  `sepia_hub` cookie. Authorize a browser once by opening
  `http://<hub>/login?token=<token>` (or any page with `?token=` — it
  plants the cookie); `GET /login` validates then redirects to `/`.
  Static assets (`/style.css`, `/manifest.json`, `/sw.js`, `/icon.svg`,
  `/pkg/*`) stay open so the service worker works pre-auth.
- `SEPIA_HOME` — the hub's `hub/` dir holds `projection.db`,
  `outbox.db`, `push.json`. State survives restarts.
- `LEPTOS_SITE_ROOT`/`SEPIA_SITE_ROOT`, `LEPTOS_ENV` (`prod`).

The hub owns the browser's push subscription store (one VAPID pair
regardless of node count) and the offline write queue: writes to a
down node land in the outbox and drain per-session-FIFO on reconnect.
Reads always come from the local projection, so the list stays instant
and works read-only while nodes are down.

**Auth.** Browser → hub auth is the hub's own token surface:
`SEPIA_HUB_TOKEN` (above) gates everything except static assets — one
`?token=` visit or `GET /login?token=…` plants an httpOnly cookie and
the browser stores nothing else. Node bearer tokens stay server-side
(`SEPIA_NODES` `@token` or `SEPIA_NODE_TOKEN`) — the browser never
stores them.

## PWA + push

`sepia-hub` serves `manifest.json`, `sw.js`, `icon.svg` — installing
the PWA gives the offline shell (last projection, read-only). Enable
push under Settings → Notifications: the browser subscribes once to
the hub; each node's `runFinished`/`permissionRequested` feed markers
fan out to every subscription (node-agnostic — one VAPID pair).

iOS requires the PWA installed on the home screen (iOS ≥ 16.4) for web
push; plain Safari tabs don't get it.

## Drivers

Each integration is a standalone binary speaking ndjson JSON-RPC over
stdio — `session.list/get/patch`, `history`, `checkpoints`,
`truncate` (rewind), `convert.import/export`, `manifest` — gated by a
`capabilities` manifest. The node exposes an agent only when both the
driver binary and its agent CLI resolve.

| Driver                | Store                                       | Agent CLI             |
| --------------------- | ------------------------------------------- | --------------------- |
| `sepia-driver-devin`  | `sessions.db` (read-only index + write ops) | `devin acp`           |
| `sepia-driver-cline`  | Cline task dirs + index                     | `cline --acp`         |
| `sepia-driver-claude` | `projects/*.jsonl` transcripts              | `claude-agent-acp`    |
| `sepia-driver-cursor` | `store.db` + transcripts                    | — (list/history only) |

`sepia driver list` shows what's discovered; installing an upgrade is
dropping a newer binary into the driver dir.

## Pairing

```bash
sepia pair        # on the node — mints a one-time code
```

The code + node URL redeem through the hub's pair flow (or
`POST /api/pair` directly) for a bearer token, then the token goes in
`SEPIA_NODES` as `id=url@token`.

## Service units

`sepia service install|uninstall|status|logs` manages launchd (macOS)
and systemd --user (Linux) units for the daemon.
