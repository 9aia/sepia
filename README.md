# Sepia

A self-hosted web app and toolkit for coding-agent sessions. Sepia lists your
Devin and Cline sessions, spawns an ACP agent per session, and lets you chat
with them from a browser — plus a CLI for converting sessions between tools.

The web app is an installable PWA: it serves a manifest + service worker, and
can push browser notifications when a run finishes or an agent needs a
permission decision.

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
  read-only; `{ "takeover": true }` SIGTERMs the lock-holder pid the agent
  reported, then loads — a takeover that still can't load fails `409 locked`
  rather than silently degrading to read-only.
- **Prompt** — `POST /api/sessions/:id/prompt`, or the AG-UI endpoint
  `POST /api/agent?sessionId=<id>` (what the chat UI uses). Concurrent prompts
  on one session return `409 busy`.
- **Cancel / permission** — `POST .../cancel` and `POST .../permission` route
  turn cancellation and tool-permission decisions.
- **History** — `GET /api/sessions/:id/history` returns the tail of the
  stored backlog; `?limit=` caps it and `?before=<index>` pages backwards
  (the `start` field is the next cursor).
- **Stream** — `GET /api/sessions/:id/stream` is a raw AG-UI SSE feed.
- **Meta** — `PATCH /api/sessions/:id` writes title/pin/project membership to
  the meta store; `DELETE` removes it (tolerant of a session the store never
  flushed). `POST .../convert` rewrites the session into another agent's
  format.
- **Projects** — `GET/POST /api/projects` + `PATCH/DELETE /api/projects/:id`
  group sessions into named projects.
- **UI config** — `GET /api/config` + `PATCH /api/config/:key` persist app
  state (section collapse, expanded dirs) server-side. Internal keys (VAPID,
  push subscriptions) are filtered from both.
- **Push** — `GET /api/push/vapid` + `POST/DELETE /api/push/subscribe` register
  Web Push subscriptions; the server fans out session events (run finished,
  permission requested) to subscribers per their per-event prefs, and prunes
  dead endpoints.

Session ids collide across agents (devin and cline mint their own), so every
session-scoped route also accepts `?agent=<id>` to scope resolution —
`attach`, `history`, `prompt`, `cancel`, `permission`, `stream`, `patch`,
`convert`, `delete`, and `POST /api/agent`. `POST /api/sessions` returns
`{ id, agentId }`; a created-but-unflushed session is remembered in the meta
store so it stays listable and deletable after a restart.

## The web app

- **Sidebar** — pinned / projects / sessions (MRU) / folders sections, all
  collapsible; a virtualized folder tree grouped by cwd; hover quick-actions
  (pin, ⋯ menu, +); filter/search/sort in the header.
- **Chat** — live AG-UI stream rendered as messages, reasoning and tool rows;
  a "Session context" card parses the agent's system prompt (`<system_info>`,
  `<rules>`) instead of dumping it as bubbles; optimistic user messages; a
  take-over confirm when the session is held by another process; earlier
  history lazy-loads on scroll.
- **Settings** — agent + model prefs, rebindable/disableable keyboard
  shortcuts (recorded via TanStack Hotkeys), push-notification toggles per
  event type, and a dark/light/system theme.
- **State** — server-reachability indicator in the sidebar footer and an
  offline screen with retry when the API is unreachable.

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
bun apps/sepia/src/main.ts export <session-id> ./out.json --db <db>
bun apps/sepia/src/main.ts install <devin-session-id> --db <db> --data-dir ~/.cline/data
bun apps/sepia/src/main.ts list --claude-dir ~/.claude
bun apps/sepia/src/main.ts install <id> --from claude --to cursor --cursor-dir ~/.cursor
```

Every verb works across the four stores — devin (`--db`), cline
(`--data-dir`), claude (`--claude-dir`), cursor (`--cursor-dir`) — selected
by `--from`/`--to` or inferred from a `--*-dir` flag. `list` reads a store;
`export` writes the session IR JSON (`--format cline` writes Cline session
files instead); `import` reads a Cline session dir, a Claude `.jsonl`
transcript or a session JSON into a store (devin by default); `install`
copies a stored session into a store ready to resume (cline by default, so
`cline --id <id>` picks it up); `delete` removes a session.

## Development

```bash
vp run ready   # check + test + build
vp check       # format + lint + typecheck
vp run -r test
vp run -r build
```

`apps/server` also has an end-to-end test (`bun apps/server/tests/e2e.ts`,
chained into its `test` script) that drives a real `Bun.serve` + control plane
against a fixture ACP agent. `apps/web` has a vitest suite for the
history/row/live-message logic (`src/tests/`).

See `AGENTS.md` for the constraints that matter to contributors, and
`DEPLOY.md` for self-hosting.
