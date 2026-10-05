# Deploying sepia

Sepia is a self-hosted control plane for coding-agent sessions. Packaged
forms: the **`sepia-node` npm package** (`npm i -g sepia-node` → `sepia
serve`, see [npm packages](#npm-packages)) and a **single `bun --compile`
binary** built from source (see [Single binary](#single-binary)). Both serve
the API and the web UI on one port — "one binary per machine, any machine
hosts the UI". For development the two halves still run side by side:

- **API** (`sepia-server`, Bun, `:8787`) — REST + AG-UI SSE + AG-UI agent endpoint
  runtime. Spawns `devin acp` / `cline --acp` subprocesses that can read and
  modify files in session working directories.
- **Web** (`sepia-web`, TanStack Start, `:3000`) — the client UI. Proxies
  `/api` to the API.

Both processes must run under **Bun ≥ 1.3**: `sepia-core` imports `bun:sqlite`
at module load, so Node cannot host the API or the repository layer.

## Single binary

```bash
vp run build:binary        # bun tools/build-binary.ts
./sepia serve              # API + embedded UI on :8787
./sepia serve --no-ui      # API-only node (same as SEPIA_UI=off)
```

`build:binary` builds the web app (TanStack Start SPA mode — the UI is a
client-rendered static bundle), stages `apps/web/dist/client` at
`apps/server/ui-dist/`, regenerates `src/ui.assets.gen.ts` with one
`import ... with { type: "file" }` per asset, and runs
`bun build apps/sepia/src/main.ts --compile`. The assets land in the binary's
`$bunfs` store and are served same-origin at `/`; every non-`/api` path falls
back to `index.html` for client routing.

Flags: `--skip-web-build` reuses an existing `dist/client`, `--outfile <path>`
renames the output (default `./sepia`).

All `SEPIA_*` configuration applies unchanged. `SEPIA_UI=off` (or `0`/`false`)
disables UI serving for API-only nodes; `SEPIA_UI_DIR=<dist dir>` serves a
web bundle from disk instead of the embedded one — useful for trying a newer
UI without rebuilding the binary.

## npm packages

The npm distribution is **source + dist**, not the compiled binary: a
`bun --compile` artifact is ~106 MiB and per-platform, while the packages
below are platform-neutral and a few MB. **Bun is the runtime dependency** —
install it with `curl -fsSL https://bun.sh/install.sh | bash`.

- **`sepia-node`** — the node + CLI. `bin/sepia` is a `#!/usr/bin/env bun`
  shim that defaults `SEPIA_UI_DIR` to the packaged web bundle (`ui/`) and
  hands off to `dist/cli.js` — one `bun build --target bun --minify` bundle
  of `apps/sepia/src/main.ts` with every workspace dep inlined (no
  `workspace:*` leaks into the manifest; `bun:*` builtins stay external).
  `npm i -g sepia-node` → `sepia serve`; `sepia version` prints the stamp.

- **`sepia-ui`** — the standalone web bundle at `dist/` for running the
  client apart from the node: `npx serve dist` (with SPA fallback to
  `index.html`), or point a node's `SEPIA_UI_DIR` at it.

Release versioning is a continuous datetime stamp — `MAJOR.YYMMDD.HHMM` UTC
(e.g. `0.261005.1330`; `0.x` = unstable, HHMM unpadded since semver forbids
leading zeros). `vp run version:bump` (=`bun tools/version.ts`) writes the
stamp into `VERSION` and every workspace `package.json`, then re-syncs
`bun.lock`. `vp run build:npm` (=`bun tools/build-npm.ts`) builds the web
bundle, bundles the CLI, and stages `packages/sepia-node/{bin,dist,ui}` and
`packages/sepia-ui/dist`.

```bash
vp run version:bump      # stamp 0.YYMMDD.HHMM everywhere
vp run build:npm         # build + stage both packages
cd packages/sepia-node && bun pm pack --destination /tmp
cd ../sepia-ui && bun pm pack --destination /tmp
cd /tmp && npm publish sepia-node-*.tgz --access public
npm publish sepia-ui-*.tgz --access public
```

Publishing from inside the workspace hits `EBADDEVENGINES` (the root
manifest pins `devEngines.packageManager: bun`), so pack with `bun pm pack`
and run `npm publish` on the tarballs from outside the repo.

`.github/workflows/release.yml` automates exactly this (stamp → `bun run
ready` → stage → `bun pm pack` → `npm publish --provenance`) on manual
`workflow_dispatch` with an `NPM_TOKEN` secret — nothing publishes until it
is dispatched.

## Security model — read this first

The API executes real coding agents. Anyone who can reach it can create a
session in an arbitrary working directory and prompt an agent to modify files.
Treat network access to the API as remote code execution.

- The API binds `127.0.0.1` by default and refuses a non-loopback
  `SEPIA_HOST` unless `SEPIA_TOKEN` is set.
- Always set `SEPIA_TOKEN` and keep it server-side. The intended topology is a
  reverse proxy that terminates TLS and injects
  `Authorization: Bearer <token>` when forwarding `/api`, so the token never
  reaches the client (see `docker-compose.yml` + `Caddyfile`).
- Deployments without a token-injecting proxy still work: the web UI shows a
  token gate on 401 and stores the token in `localStorage` (`sepia:token`,
  bound to the node address it was entered for — repointing the client at a
  different node re-prompts rather than replaying the credential elsewhere).
  It is sent as `Authorization: Bearer` on API calls and as `?access_token=`
  on the two SSE streams (`/api/events`, `/api/sessions/:id/stream` —
  EventSource cannot set headers, and the server only honors the query
  credential on those GETs). The access log only records `url.pathname`, so
  the token never appears in logs.
- CORS is not the gate — it only affects browsers. Auth applies to every
  `/api/*` route except `GET /api/health` and `POST /api/pair` (the pairing
  bootstrap, authorized by the one-time code).
- The server opens the Devin store **read-only**; session writes happen inside
  the agent CLIs, not sepia.

## Configuration (API)

| Variable                      | Default                                          | Purpose                                                                      |
| ----------------------------- | ------------------------------------------------ | ---------------------------------------------------------------------------- |
| `SEPIA_HOST`                  | `127.0.0.1`                                      | Bind address. Non-loopback requires `SEPIA_TOKEN`.                           |
| `PORT`                        | `8787`                                           | API port.                                                                    |
| `SEPIA_TOKEN`                 | unset                                            | Bearer token required on all `/api/*` routes when set.                       |
| `SEPIA_DB`                    | `~/.local/share/devin/cli/sessions.db`           | Devin session store path (opened read-only).                                 |
| `SEPIA_CLINE_DIR`             | `~/.cline/data`                                  | Cline data dir merged into the session list (read-only overlay).             |
| `SEPIA_CLAUDE_DIR`            | `~/.claude`                                      | Claude Code dir; `<dir>/projects` merged into the session list.              |
| `SEPIA_CURSOR_DIR`            | `~/.cursor`                                      | Cursor dir; `chats/` + `projects/` merged into the session list (read-only). |
| `SEPIA_ORIGINS`               | `http://localhost:3000,http://127.0.0.1:3000`    | Comma-separated CORS allowlist for browser calls.                            |
| `SEPIA_UI`                    | `on`                                             | `off`/`0`/`false` disables static UI serving (API-only node).                |
| `SEPIA_UI_DIR`                | unset                                            | Serve a web bundle from this dir instead of the embedded one.                |
| `SEPIA_AGENT_<ID>_COMMAND`    | `devin acp` / `cline --acp` / `claude-agent-acp` | Override the spawn argv per agent id (space-separated).                      |
| `SEPIA_IDLE_TTL_MS`           | `600000`                                         | Detach live sessions idle this long; `0` disables.                           |
| `SEPIA_SWEEP_MS`              | `30000`                                          | Idle-sweep interval.                                                         |
| `SEPIA_LOCK_TTL_MS`           | `5000`                                           | Lock-probe result cache.                                                     |
| `SEPIA_HELD_WATCH_MS`         | `5000`                                           | Re-probe interval for held sessions feeding `/api/events`; `0` disables.     |
| `SEPIA_META`                  | `~/.local/share/sepia/meta.json`                 | Sepia-owned session metadata (title overrides via PATCH).                    |
| `SEPIA_HISTORY_LIMIT`         | `500`                                            | Default tail limit for `GET .../history`.                                    |
| `SEPIA_SSE_KEEPALIVE_MS`      | `15000`                                          | SSE keep-alive frame interval; `0` disables.                                 |
| `SEPIA_INHERIT_ENV`           | unset                                            | `1` forwards the whole parent env to agents (allowlist otherwise).           |
| `SEPIA_DEBUG`                 | unset                                            | `1` streams agent stderr into the server log.                                |
| `SEPIA_OTEL`                  | `1`                                              | `0` disables OTLP telemetry export.                                          |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318`                          | OTLP/HTTP collector endpoint (LGTM in `lgtm/`).                              |
| `OTEL_SERVICE_NAME`           | `sepia-server`                                   | OTel resource service name.                                                  |

## Agent authentication

Sessions run inside the agent CLI, which needs its own credentials:

- **Devin**: `devin auth login` on the host (or `WINDSURF_API_KEY`, which is
  forwarded to agent children through the env allowlist).
- **Cline**: `cline --acp` uses the Cline CLI's own auth state.
- **Claude Code**: `claude` has no native ACP mode — sepia spawns
  `claude-agent-acp` (`npm i -g @agentclientprotocol/claude-agent-acp`),
  which supports `session/load`/`session/list` over the JSONL transcripts.
  It uses the Claude Code login under `~/.claude` (`HOME` is forwarded);
  `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` pass the env allowlist.
  Without the adapter installed, Claude sessions still list and page
  history — attach fails at spawn.

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
