# Sepia

A self-hosted client + node for coding-agent sessions. Sepia lists sessions
from every connected machine — Devin, Cline, Claude Code and Cursor — drives
them live over the Agent Client Protocol (ACP), and lets a session move
between machines and agents: stop on one, resume on another.

```
┌─ Sepia Client (apps/web — TanStack Start PWA) ────────────────┐
│  runs in a browser, on any device. holds the peer registry +   │
│  a client identity (label + keypair). merges every connected   │
│  node's sessions/projects/agents; actions go to the node that  │
│  holds each session's lock.                                    │
└───────┬───────────────┬───────────────┬───────────────────────┘
        │ HTTP+SSE      │ HTTP+SSE      │ HTTP+SSE (direct or gateway)
        ▼               ▼               ▼
   Sepia Node      Sepia Node      Sepia Node
   (sepia serve)   (sepia serve)   (sepia serve — unreachable nodes
        │               │          route through a peer's gateway)
        ▼               ▼               ▼
   session stores + ACP runtimes (devin acp, cline --acp, claude-agent-acp)
```

## Concepts

**Sepia Protocol** — the data model + API surface nodes speak:

- **Session IR** (`packages/sepia`) — the canonical intermediate
  representation every agent store reads and writes: messages with roles and
  content blocks (text, thinking + signature, tool calls, images, file
  attachments), tool calls with args/results/status/diffs/durations,
  per-message token usage + cost, sub-agent lineage (`parentSessionId`),
  checkpoints (file diffs, git commits, Cline shadow-git refs), and the meta
  overlay (title/pin/projects/spans/tags — Sepia-side metadata the agent
  stores can't carry). A _run span_ `{agent, node, at}` marks which agent on
  which machine produced each stretch — a session is a container; agent and
  machine are provenance, not identity.
- **The API** — REST + AG-UI SSE over `/api/*`: sessions
  (list/attach/prompt/cancel/permission/history/convert/restore/rewind/
  import), projects, meta, `/api/node` (a node's descriptor), `/api/events`
  (the node's SSE feed — session/meta/project/busy diffs),
  `/api/gateway/:serverId/*` (proxy through a peer for unreachable targets),
  `/api/pair` (redeem a `sepia pair` code for a bearer token), managed
  servers (`/api/servers` — the node's sealed credential registry + SSH
  tunnels), push (`/api/push/*`).

**Sepia Node** — one `sepia serve` process: the API plus the session stores
and agent runtimes on that machine. Bundles to a single `bun --compile`
binary that serves the client too. Each node has an id, name, agent roster
and capabilities (`GET /api/node`); it owns session locks, emits `/api/events`,
stores managed-server credentials encrypted (AES-256-GCM), and can gateway
calls to peers the client can't reach.

**Sepia Client** — the web UI (`apps/web`): a client container, not a node.
It holds the node registry (localStorage), a client identity (label +
Ed25519/ECDSA keypair, regenerated on demand), and no session data of its
own — sessions, projects and agents merge from every connected node; node
keys (`node:agent:id`) keep ids collision-safe. A disconnected client shows
"no nodes connected"; each node degrades independently. The footer picks a
_focus_ (`node · agent`) that drives session creation.

## Packages

| Path                       | Package                 | Role                                                                         |
| -------------------------- | ----------------------- | ---------------------------------------------------------------------------- |
| `packages/sepia`           | `sepia-core`            | Session IR, Devin/Cline/Claude/Cursor stores, convert, restore, rewind       |
| `packages/acp`             | `sepia-acp`             | Spawn an ACP agent over stdio; typed session ops + normalized updates        |
| `packages/agui`            | `sepia-agui`            | Translate ACP session updates into AG-UI events; SSE encoding                |
| `packages/session-control` | `sepia-session-control` | Control plane: session registry, live-agent ownership, locks, restore exec   |
| `apps/sepia`               | `sepia-cli`             | CLI (`list`, `export`, `import`, `install`, `delete`) across all four stores |
| `apps/server`              | `sepia-server`          | Bun API: the node — REST + AG-UI SSE + events feed + gateway + push          |
| `apps/web`                 | `sepia-web`             | TanStack Start client (PWA, AI Elements chat)                                |

## Quickstart

Requires Bun ≥ 1.3 and `vp` (Vite+). The agent CLIs must be installed and
authenticated (`devin acp`, `cline --acp`, `claude-agent-acp`).

```bash
vp install

# terminal 1 — the node: API on 127.0.0.1:8787
bun apps/server/src/main.ts

# terminal 2 — the client: web on :3000 (proxies /api to :8787)
vp run dev
```

Open http://localhost:3000, pick a session, and chat. The "New session"
form spawns an agent in a working directory you choose.

For a single-machine deploy, `./sepia serve` (the compiled binary) serves
client + API on one port; for multi-machine, run it on each machine and
pair them from Settings → Nodes (`sepia pair` on the remote machine mints a
one-time code).

## How sessions behave

- **List** — `GET /api/sessions?withLocks=1` merges the persisted stores
  with live sessions and probes lock state (bounded by `SEPIA_LOCK_TTL_MS`).
- **Create** — `POST /api/sessions { cwd, agent?, title? }` → `session/new`.
- **Attach** — `POST /api/sessions/:id/attach` spawns an agent and
  `session/load`s the session. A session locked by a live process attaches
  read-only; `{ "takeover": true }` SIGTERMs the lock-holder pid, then
  loads — a takeover that still can't load fails `409 locked` rather than
  silently degrading. A held session's live transcript keeps streaming to
  watchers (the node diffs it via the held-session watch).
- **Prompt** — `POST /api/sessions/:id/prompt` accepts text + content blocks
  (images, files), gated by the agent's `promptCapabilities`. `POST
/api/agent?sessionId=<id>` is the AG-UI path the chat uses. Concurrent
  prompts on one session return `409 busy`.
- **Cancel / permission** — `POST .../cancel` and `POST .../permission` route
  turn cancellation and tool-permission decisions.
- **History** — `GET /api/sessions/:id/history?limit=&before=` pages the
  stored backlog backwards.
- **Stream** — `GET /api/sessions/:id/stream` is a raw AG-UI SSE feed.
- **Resume/convert** — `POST .../convert` rewrites the session into another
  agent's format in place; `POST .../resume` (or the client's "Resume on…")
  exports the session IR and rebuilds it on a target node/agent — tool-call
  ids, thinking, usage and lineage all preserved.
- **Restore / rewind** — `POST .../restore` reverts files to a checkpoint
  (per-diff revert for Devin, shadow-git for Cline); `POST .../rewind`
  truncates the transcript to a point (each store's native semantics).
- **Meta** — `PATCH /api/sessions/:id` writes title/pin/project/spans;
  `DELETE` removes the session.
- **Projects** — `GET/POST/PATCH/DELETE /api/projects[/:id]` group sessions.
- **Push** — `GET /api/push/vapid` + `POST/DELETE /api/push/subscribe`
  register Web Push subscriptions; the server fans out session events per
  per-event prefs and prunes dead endpoints.

Every session-scoped route accepts `?agent=<id>` (session ids collide across
agents) and, on a federated client, routes to the session's node.

## The client

- **Sidebar** — pinned / projects / sessions / folders sections (configurable:
  reorder, relabel, disable via ReUI sortable), virtualized folder tree grouped
  by cwd and node, filter/search/sort, node badges and lock indicators.
- **Chat** — live AG-UI stream rendered as messages, reasoning, per-tool
  detail renderers (`$ command` + output, `+/-` diff blocks, path chips,
  search counts) inside ReUI Message/Tool/CodeBlock components; usage footer
  (`↑in ↓out · cost`); attachments (paste/drop/paperclip → ACP content
  blocks); checkpoint restore and rewind actions; context tabs (reports,
  system prompt, rules, skills) parsed from the transcript; draft-while-held
  → take-over-and-send.
- **Settings** — General (theme, per-node creation defaults), Client (label +
  keypair), Sidebar section, Models (per-agent prefs), Nodes (peer registry:
  add/pair/edit/enable/gateway — a gateway node's edit dialog carries its
  managed credential's secret + SSH tunnel), Credentials (labeled token
  store nodes reference), Keyboard, Notifications.
- **Focus** — the footer's `node · agent` pick drives what "new session"
  means; per-node defaults apply per machine since agent ids and cwds are
  local to it.

## Security

**The API spawns coding agents that read and modify files.** Anyone who can
reach it can run code. It binds `127.0.0.1` by default and refuses a public
bind without `SEPIA_TOKEN`; when set, every `/api/*` route except
`GET /api/health` requires `Authorization: Bearer <token>`. Pairing
(`sepia pair`) mints short-lived one-time codes redeemable for a bearer.
Managed-server credentials are AES-256-GCM sealed at rest (`servers.json`,
key in `~/.config/sepia/`). See `DEPLOY.md` for the full env reference and
deploy topologies.

## CLI

```bash
bun apps/sepia/src/main.ts list --db ~/.local/share/devin/cli/sessions.db
bun apps/sepia/src/main.ts export <session-id> ./out.json --db <db>
bun apps/sepia/src/main.ts import <path-or-dir> --db <db>
bun apps/sepia/src/main.ts install <id> --from devin --to cline --data-dir ~/.cline/data
bun apps/sepia/src/main.ts install <id> --from claude --to cursor --cursor-dir ~/.cursor
bun apps/sepia/src/main.ts delete <id> --claude-dir ~/.claude
```

Every verb works across the four stores — devin (`--db`), cline
(`--data-dir`), claude (`--claude-dir`), cursor (`--cursor-dir`) — selected
by `--from`/`--to` or inferred from a `--*-dir` flag. `export` writes the
session IR JSON (`--format cline` writes Cline session files instead);
`install` copies a stored session into a target store ready to resume.

## Development

```bash
vp run ready   # check + test + build
vp check       # format + lint + typecheck
vp run -r test
vp run -r build
```

`apps/server` has an end-to-end test (`bun apps/server/tests/e2e.ts`) that
drives a real `Bun.serve` + control plane against a fixture ACP agent.
`apps/web` has a vitest suite for history/rows/live-message/federation logic.

See `AGENTS.md` for the constraints that matter to contributors, `DEPLOY.md`
for self-hosting, `docs/session-formats.md` for what each agent's store
actually looks like, and `docs/protocol.md` for the federation protocol.
