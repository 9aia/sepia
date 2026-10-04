# Sepia

A self-hosted web app and toolkit for coding-agent sessions. Sepia lists your
Devin and Cline sessions, spawns an ACP agent per session, and lets you chat
with them from a browser — plus a CLI for converting sessions between tools.

```
apps/web (TanStack Start + AI Elements)
  Conversation/PromptInput ──▶ /api/sessions/:id/{prompt,stream}   REST + AG-UI SSE
                                  └─▶ sepia-session-control
                                        ├─▶ sepia-core  (IR + Devin/Cline stores)
                                        ├─▶ sepia-acp   ──stdio JSON-RPC──▶ devin acp / cline --acp
                                        └─▶ sepia-agui  (ACP → AG-UI translation)

  Also public for AG-UI clients: POST /api/agent (RunAgentInput → SSE)
```

## Packages

| Path                       | Package                 | Role                                                                  |
| -------------------------- | ----------------------- | --------------------------------------------------------------------- |
| `packages/sepia`           | `sepia-core`            | Session IR, Devin/Cline adapters, stores, conversion                  |
| `packages/acp`             | `sepia-acp`             | Spawn an ACP agent over stdio; typed session ops + normalized updates |
| `packages/agui`            | `sepia-agui`            | Translate ACP session updates into AG-UI events; SSE encoding         |
| `packages/session-control` | `sepia-session-control` | Control plane: lists sessions, owns one live agent per session, locks |
| `apps/sepia`               | `sepia-cli`             | CLI (`list`, `import`, `export`, `install`)                           |
| `apps/server`              | `sepia-server`          | Bun API: REST + AG-UI SSE + AG-UI agent endpoint                      |
| `apps/web`                 | `sepia-web`             | TanStack Start UI (AI Elements chat)                                  |

## Quickstart

Requires Bun ≥ 1.3 and `vp` (Vite+). The agent CLIs must be installed and
authenticated (`devin acp`, `cline --acp`).

```bash
vp install

# terminal 1 — API on 127.0.0.1:8787
bun apps/server/src/main.ts

# terminal 2 — web on :3000 (proxies /api to :8787)
vp run dev
```

Open http://localhost:3000, pick a session, and chat. The "New session" form
spawns an agent in a working directory you choose.

## How sessions behave

- **List** — `GET /api/sessions` merges the persisted store (`sepia-core`) with
  live sessions; `?withLocks=1` probes an agent for lock state.
- **Create** — `POST /api/sessions { cwd, agent?, title? }` → `session/new`.
- **Attach** — `POST /api/sessions/:id/attach` spawns an agent and
  `session/load`s the session. A session locked by a live process attaches
  read-only; `{ "takeover": true }` overrides.
- **Prompt** — `POST /api/sessions/:id/prompt`, or the AG-UI endpoint
  `POST /api/agent?sessionId=<id>` (what the chat UI uses). Concurrent prompts
  on one session return `409 busy`.
- **Cancel / permission** — `POST .../cancel` and `POST .../permission` route
  turn cancellation and tool-permission decisions.
- **History** — `GET /api/sessions/:id/history` returns the tail of the
  stored backlog; `?limit=` caps it and `?before=<index>` pages backwards
  (the `start` field is the next cursor).
- **Stream** — `GET /api/sessions/:id/stream` is a raw AG-UI SSE feed.

## Security

**The API spawns coding agents that read and modify files.** Anyone who can
reach it can run code. It binds `127.0.0.1` by default and refuses a public
bind without `SEPIA_TOKEN`; when the token is set, every `/api/*` route except
`/api/health` requires `Authorization: Bearer <token>`. See `DEPLOY.md` for
the token-injecting reverse-proxy topology and the full env reference.

## CLI

```bash
bun apps/sepia/src/main.ts list --db ~/.local/share/devin/cli/sessions.db
bun apps/sepia/src/main.ts import ~/.cline/data/sessions/<id> --db <db>
bun apps/sepia/src/main.ts export <devin-session-id> ./out --db <db>
bun apps/sepia/src/main.ts install <devin-session-id> --db <db> --data-dir ~/.cline/data
```

`import`/`export` convert between Cline and Devin session formats; `install`
places a Devin session into the Cline store so `cline --id <id>` resumes it.

## Development

```bash
vp run ready   # check + test + build
vp check       # format + lint + typecheck
vp run -r test
vp run -r build
```

`apps/server` also has an end-to-end test (`bun apps/server/tests/e2e.ts`,
chained into its `test` script) that drives a real `Bun.serve` + control plane
against a fixture ACP agent.

See `AGENTS.md` for the constraints that matter to contributors, and
`DEPLOY.md` for self-hosting.
