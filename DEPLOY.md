# Deploying sepia

Sepia is a self-hosted control plane for coding-agent sessions. Two processes
run side by side:

- **API** (`sepia-server`, Bun, `:8787`) — REST + AG-UI SSE + AG-UI agent endpoint
  runtime. Spawns `devin acp` / `cline --acp` subprocesses that can read and
  modify files in session working directories.
- **Web** (`sepia-web`, TanStack Start, `:3000`) — the browser UI. Proxies
  `/api` to the API.

Both processes must run under **Bun ≥ 1.3**: `sepia-core` imports `bun:sqlite`
at module load, so Node cannot host the API or the repository layer.

## Security model — read this first

The API executes real coding agents. Anyone who can reach it can create a
session in an arbitrary working directory and prompt an agent to modify files.
Treat network access to the API as remote code execution.

- The API binds `127.0.0.1` by default and refuses a non-loopback
  `SEPIA_HOST` unless `SEPIA_TOKEN` is set.
- Always set `SEPIA_TOKEN` and keep it server-side. The intended topology is a
  reverse proxy that terminates TLS and injects
  `Authorization: Bearer <token>` when forwarding `/api`, so the token never
  reaches the browser (see `docker-compose.yml` + `Caddyfile`).
- Deployments without a token-injecting proxy still work: the web UI shows a
  token gate on 401 and stores the token in `localStorage` (`sepia:token`).
  It is sent as `Authorization: Bearer` on API calls and as `?access_token=`
  on the SSE stream (EventSource cannot set headers). The access log only
  records `url.pathname`, so the token never appears in logs.
- CORS is not the gate — it only affects browsers. Auth applies to every
  `/api/*` route except `GET /api/health`.
- The server opens the Devin store **read-only**; session writes happen inside
  the agent CLIs, not sepia.

## Configuration (API)

| Variable                      | Default                                       | Purpose                                                            |
| ----------------------------- | --------------------------------------------- | ------------------------------------------------------------------ |
| `SEPIA_HOST`                  | `127.0.0.1`                                   | Bind address. Non-loopback requires `SEPIA_TOKEN`.                 |
| `PORT`                        | `8787`                                        | API port.                                                          |
| `SEPIA_TOKEN`                 | unset                                         | Bearer token required on all `/api/*` routes when set.             |
| `SEPIA_DB`                    | `~/.local/share/devin/cli/sessions.db`        | Devin session store path (opened read-only).                       |
| `SEPIA_CLINE_DIR`             | `~/.cline/data`                               | Cline data dir merged into the session list (read-only overlay).   |
| `SEPIA_ORIGINS`               | `http://localhost:3000,http://127.0.0.1:3000` | Comma-separated CORS allowlist for browser calls.                  |
| `SEPIA_AGENT_<ID>_COMMAND`    | `devin acp` / `cline --acp`                   | Override the spawn argv per agent id (space-separated).            |
| `SEPIA_IDLE_TTL_MS`           | `600000`                                      | Detach live sessions idle this long; `0` disables.                 |
| `SEPIA_SWEEP_MS`              | `30000`                                       | Idle-sweep interval.                                               |
| `SEPIA_LOCK_TTL_MS`           | `5000`                                        | Lock-probe result cache.                                           |
| `SEPIA_HISTORY_LIMIT`         | `500`                                         | Default tail limit for `GET .../history`.                          |
| `SEPIA_SSE_KEEPALIVE_MS`      | `15000`                                       | SSE keep-alive frame interval; `0` disables.                       |
| `SEPIA_INHERIT_ENV`           | unset                                         | `1` forwards the whole parent env to agents (allowlist otherwise). |
| `SEPIA_DEBUG`                 | unset                                         | `1` streams agent stderr into the server log.                      |
| `SEPIA_OTEL`                  | `1`                                           | `0` disables OTLP telemetry export.                                |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318`                       | OTLP/HTTP collector endpoint (LGTM in `lgtm/`).                    |
| `OTEL_SERVICE_NAME`           | `sepia-server`                                | OTel resource service name.                                        |

## Agent authentication

Sessions run inside the agent CLI, which needs its own credentials:

- **Devin**: `devin auth login` on the host (or `WINDSURF_API_KEY`, which is
  forwarded to agent children through the env allowlist).
- **Cline**: `cline --acp` uses the Cline CLI's own auth state.

## Docker / compose

`Dockerfile` builds the workspace and produces one image used by both
services. `docker-compose.yml` runs `api` + `web` behind a `caddy` proxy that
injects the bearer token:

```bash
SEPIA_TOKEN=$(openssl rand -hex 32) docker compose up --build
# UI on http://localhost:8080
```

Mount your Devin store (read-only) and make agent CLIs reachable in the
container — either bake them into a derived image or point
`SEPIA_AGENT_<ID>_COMMAND` at commands that are. Sessions created by agents
live in the agent CLI's store; mount `~/.local/share/devin` (not just the db)
if agents must persist credentials/sessions.

## Reverse proxy (non-Docker)

Run the API and web locally, then proxy `/api` with token injection. nginx:

```nginx
location /api/ {
  proxy_pass http://127.0.0.1:8787;
  proxy_set_header Authorization "Bearer $SEPIA_TOKEN";
  proxy_set_header Connection "";
  proxy_http_version 1.1;
  # SSE needs no buffering
  proxy_buffering off;
}
location / {
  proxy_pass http://127.0.0.1:3000;
}
```

Terminate TLS at the proxy — the API itself speaks plain HTTP.

## systemd

```ini
[Unit]
Description=sepia API
After=network.target

[Service]
Environment=SEPIA_TOKEN=<token>
Environment=SEPIA_HOST=127.0.0.1
ExecStart=/home/you/.bun/bin/bun /opt/sepia/apps/server/src/main.ts
WorkingDirectory=/opt/sepia
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

SIGINT/SIGTERM/SIGHUP all trigger a graceful shutdown that closes agent
subprocesses (releasing their session locks), so plain `systemctl stop` is safe.

## Health and logs

- `GET /api/health` → `{"ok":true,"db":true}` (503 when the store is
  unreadable). Unauthenticated; safe for load balancers.
- Every request logs `METHOD path status ms` to stdout (`/api/health`
  excluded).
- Agent stderr is captured into a bounded buffer (`recentStderr`); stream it
  live with `SEPIA_DEBUG=1`.
