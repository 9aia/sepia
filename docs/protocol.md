# Sepia protocol v1

One binary (`sepia serve`) per machine. The UI aggregates any number of nodes
into a single view — sessions, projects, chat — and routes each action to the
node that owns the resource. There is no central server and nothing to sync:
each node is authoritative for what's on its own disk.

## Identity

`GET /api/node` → node metadata. The one endpoint every client hits first.

```json
{
  "id": "node_a3f8c2d1e7b4",
  "name": "thinkpad",
  "version": "1.0.0",
  "protocol": 1,
  "agents": ["devin", "cline"],
  "capabilities": ["sessions", "projects", "push", "events"]
}
```

- `id` — stable random string generated on first boot, stored in
  `~/.local/share/sepia/node.json` (or `$SEPIA_HOME`). Never changes.
- `name` — hostname by default, user-renameable (`PATCH /api/node`).
- `protocol` — integer; bump on breaking changes. Clients negotiate.

## Resource keys

`{node}:{agent}:{sessionId}` — three segments, colon-joined.

- `node` — the node's `id`. Always present in the aggregated UI; a single-node
  client may elide it.
- `agent` — `devin`, `cline`, future adapters. Existing `agent:id` keys keep
  working on any single node.
- `sessionId` — the agent store's own id.

Every list endpoint returns rows scoped to the node being asked — the `node`
segment is implicit in the wire format (it's the node you called). The UI
prefixes it when merging.

## API surface

Session ops (all bearer-authenticated except `GET /api/health`):

```
GET    /api/node                        node metadata
GET    /api/sessions                  session list (with meta overlay)
POST   /api/sessions                  create { cwd, title?, agent? }
GET    /api/sessions/:id/history      paginated { messages, start, hasMore }
GET    /api/sessions/:id/export       { session } — the complete session IR:
                                        nodes with toolCalls ids/args, thinking,
                                        usage, toolCallId links and the
                                        parent-linked tree; 404 on older nodes
POST   /api/sessions/:id/attach       attach live control { model?, takeover? }
POST   /api/sessions/:id/prompt       send { text }
POST   /api/sessions/:id/cancel       stop the run
POST   /api/sessions/:id/permission   reply to a pending permission
PATCH  /api/sessions/:id              meta overlay { title?, pinned?, archived?, projectIds?, model? }
DELETE /api/sessions/:id
POST   /api/sessions/:id/convert      { agent } → new session in another agent's store
POST   /api/sessions/import           { agent, cwd?, title?, session | history } → session summary
                                        (the "Resume on…" write — convert with
                                        explicit IR. {session} is the /export
                                        payload verbatim — full fidelity;
                                        {history} is the flat compat form for
                                        older source nodes)
GET    /api/sessions/:id/stream       AG-UI SSE (live run)
GET    /api/events                    node event feed (see below)
POST   /api/pair                      { code } → { token } — unauthenticated
                                      bootstrap; the one-time code authorizes it
GET    /api/projects                  node-local projects
POST   /api/projects                  { name } → project
PATCH  /api/projects/:id              rename
DELETE /api/projects/:id
GET    /api/config/:key  PATCH /api/config/:key   server-side UI state
GET    /api/push/vapid  POST/DELETE /api/push/subscribe   web-push
```

`?agent=` disambiguates a bare `:id` across agent stores on one node.

## The node event feed

`GET /api/events` — one SSE stream per node, the aggregated-UI replacement for
polling:

```
event: session     data: {"id":"...","agent":"...","patch":{"busy":true}}
event: meta        data: {"id":"...","agent":"...","patch":{"pinned":false}}
event: project     data: {"id":"...","patch":{}}  // created/renamed/deleted
event: heartbeat   data: {"ts":1700000000}
```

- `session` — a summary row changed (created, updated, deleted).
- `meta` — the overlay (title/pinned/archived/projectIds/model) changed.
- `project` — a project row changed.
- `heartbeat` — keepalive every `SEPIA_SSE_KEEPALIVE_MS`.

The UI invalidates the matching queries on receipt. Nodes with no clients
still run the control plane; events are best-effort, so a missed event just
means a slightly stale row until the next refetch.

## Projects

Node-local: `{ id, name, cwd }` where `cwd` is a path on _that_ node. The
aggregated UI shows `name @ node` (or a machine badge). Cross-machine
grouping of like-named projects is a UI concern — no sync.

## Auth — pairing

Two modes, same bearer credential underneath:

1. **Direct** — set `SEPIA_TOKEN` on the node, paste the token into the UI's
   node registry. Works today.
2. **Pairing** — `sepia pair` prints a short one-time code (or QR).
   The UI posts it to `POST /api/pair` on the node and receives a long-lived
   credential. Codes expire in ~60s and are single-use. Tailscale-style
   bootstrap without SSH.

Minting is gated by the filesystem, not the network: `sepia pair` runs on
the node and writes `{code, expiresAt}` to `$SEPIA_HOME/pair-code`, which
the server consumes on the next `POST /api/pair`. Whoever can write to
`$SEPIA_HOME` is the machine owner — the right mint authority — so no
mint endpoint is exposed. Codes are Crockford base32 in a 4-4 group
(`7K2M-9PQX`), case-insensitive on input. Issued credentials are `sepia_…`
bearer tokens that authenticate exactly like `SEPIA_TOKEN`; they persist as
sha256 hashes in `$SEPIA_HOME/tokens.json`, so deleting that file revokes
them.

Credentials are stored per-node in the UI's `node → token` map
(`localStorage`/OS keychain later). CORS allows the serving origin + any
registered peer origins.

## Safety boundary

The API mutates _sessions_, not the machine: create/attach/prompt/cancel/
patch/delete on agent stores, plus meta overlay and projects. No shell, no
filesystem writes outside the stores, no arbitrary process control — that
contract is part of the protocol.

## Failure model

- A node that's down just doesn't appear — lists render from the nodes that
  answer. Timeouts are short (~3s per node) so one dead laptop doesn't stall
  the list.
- `/api/events` reconnects with backoff; a missed event is a stale row, not
  lost data.
- Optimistic mutations roll back per node — `db-query-collection`'s retry
  path covers the transient-failure case.

## Phasing

1. **Phase 1** — `GET /api/node` + UI node registry + merged lists with node
   badges + per-node action routing. Frontend + one endpoint; works on LAN.
2. **Phase 2** — `/api/events` (no polling), `sepia pair` code exchange,
   `bun --compile` binary serving the built UI + API.
3. **Phase 3** (optional) — gateway mode: one node proxies unreachable peers.
   Adds a server-side key-translation layer; defer until needed.
