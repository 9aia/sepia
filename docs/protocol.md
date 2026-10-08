# Sepia protocol v1

One node (`sepia-node`, or `sepia serve`) per machine. The UI aggregates any
number of nodes into a single view — sessions, projects, chat — and routes
each action to the node that owns the resource. There is no central server
and nothing to sync: each node is authoritative for what's on its own disk.

This document describes the Rust implementation (`crates/sepia-http` mounts
the routes; `crates/sepia-node` composes and serves them). Surfaces the
pre-Rust server carried that this one does not — the `/api/servers` registry,
`/api/gateway/*` forwarding, session `pull`/`push`, project bundles, and the
AG-UI stream — are gone; the notes below describe what exists.

## Identity

`GET /api/node` → node metadata. The one endpoint every client hits first.

```json
{
  "id": "node_a3f8c2d1e7b4",
  "name": "thinkpad",
  "version": "0.0.0",
  "protocol": 1,
  "agents": ["devin", "cline"],
  "capabilities": ["sessions", "projects", "push", "events", "export", "pairing"]
}
```

- `id` — stable `node_<hex16>` minted on first boot, stored in
  `~/.local/share/sepia/node.json` (`$SEPIA_NODE` overrides the path). Never
  changes.
- `name` — hostname by default (`$SEPIA_NAME` overrides), user-renameable
  via `PATCH /api/node` with `{ "name": "…" }`. The rename persists to the
  identity file and returns the updated descriptor (same shape as GET).
  `name` must be a non-empty string, max 100 chars.
- `version` — the build's package version stamp.
- `protocol` — integer; bump on breaking changes. Clients negotiate.
- `agents` — agent ids whose driver resolved at boot (see `GET /api/agents`).
- `capabilities` — `sessions`, `projects`, `push`, `events`, `export`, plus
  `pairing` when the pairing store is configured.

`PATCH /api/node` requires a JSON object body `{ "name": "…" }`; anything
else is a 400 `{error}`.

## Resource keys

`{node}:{agent}:{sessionId}` — three segments, colon-joined.

- `node` — the node's `id`. Always present in the aggregated UI; a single-node
  client may elide it.
- `agent` — `devin`, `cline`, `claude`, `cursor` (whatever drivers resolved).
- `sessionId` — the agent store's own id.

Every list endpoint returns rows scoped to the node being asked — the `node`
segment is implicit in the wire format (it's the node you called). The UI
prefixes it when merging. Session-scoped routes also accept `?agent=` to
disambiguate a bare `:id` that collides across agent stores on one node.

## API surface

All `/api/*` routes are bearer-authenticated (see Auth) except
`GET /api/health`, `POST /api/pair`, and the two `POST /api/auth/*` cookie
verbs. Wrong-method requests on a mounted path fall through to the generic
404 `{ "error": "Not found" }` — the surface never answers 405.

```
GET    /api/health                    { ok, db } — 200 when the session list
                                      answers within ~1.5s, else 503
GET    /api/node                      node descriptor (above)
PATCH  /api/node                      { name } — rename; persists node.json
GET    /api/user                      { user: { username, homedir, shell,
                                      hostname, platform, arch } } — the OS
                                      user the node runs as
GET    /api/fs?path=/abs/dir          { dirs } — direct subdirectories (max
                                      200), for the composer cwd picker
GET    /api/agents                    { agents: [{ id, label, capabilities? }] }
                                      — capabilities surface after a live
                                      attach advertises them
GET    /api/sessions                  { sessions } — store summaries + meta
                                      overlay + meta-only pending rows;
                                      ?withLocks=1 merges agent lock flags
POST   /api/sessions                  create — { cwd (required), agent?,
                                      title?, model?, fallbacks? } →
                                      201 { id, agentId, capabilities }
GET    /api/sessions/:id              one session row (list shape)
PATCH  /api/sessions/:id              meta overlay — { title? (non-empty,
                                      ≤200), pinned?, archived?,
                                      projectIds? (string[]), model?
                                      (string|null) } → { ok: true }
PATCH  /api/sessions/:id/meta         alias of the item PATCH
DELETE /api/sessions/:id              → { ok: true } (tolerates missing ids)
GET    /api/sessions/:id/history      paginated — ?limit, ?before (non-negative
                                      ints; default limit SEPIA_HISTORY_LIMIT)
                                      → { messages, total, start }
GET    /api/sessions/:id/checkpoints  { checkpoints } — workspace snapshot
                                      refs the store recorded
GET    /api/sessions/:id/export       { session } — the complete session IR:
                                      parent-linked nodes, tool calls,
                                      thinking, usage
GET    /api/sessions/:id/stream       session-event SSE (see below)
POST   /api/sessions/:id/attach       attach live control —
                                      { takeover?, model?, fallbacks? } →
                                      { attached, readOnly, agentId,
                                      capabilities }. A session locked by a
                                      live process attaches read-only;
                                      takeover: true signals the holder
POST   /api/sessions/:id/detach       release the live attach → { ok: true }
POST   /api/sessions/:id/prompt       { text?, attachments? } — attachments is
                                      an ACP content-block array (text, image,
                                      audio, resource, resource_link; ≤16
                                      parts) → { ok: true }
POST   /api/sessions/:id/cancel       stop the current run → { ok: true }
POST   /api/sessions/:id/permission   reply to a pending permission —
                                      { requestId, optionId? } (optionId may
                                      be null) → { ok: true };
                                      /permissions is the same route
POST   /api/sessions/:id/restore      file restore — writes under the
                                      session's cwd; requires
                                      { confirm: true }:
                                      { path, toolCallId? } reverts the file
                                        via recorded diffs (pre-session
                                        state, or just that call's change)
                                      { checkpoint, paths? } materializes the
                                        files a checkpoint ref covers
                                      → { restored: [{path,…}], skipped:
                                      [{path,reason}] }
POST   /api/sessions/:id/rewind       conversation rewind — truncates the
                                      transcript, not files (that's restore).
                                      Requires { confirm: true } plus exactly
                                      one selector: { nodeId } keeps that
                                      node and everything before it,
                                      { turns } (positive int) drops the last
                                      N user turns, { checkpoint } rewinds
                                      to a recorded snapshot ref →
                                      { kept, removed }
POST   /api/sessions/:id/convert      { agent: "cline"|"devin" } →
                                      { sessionId } — copy into another
                                      agent's store; 501 when the node's
                                      convert seam isn't wired
POST   /api/sessions/import           { agent, cwd?, title?, model?,
                                      session | history } → 201 session row
                                      (the "Resume on…" write — convert with
                                      explicit IR). {session} is the /export
                                      payload verbatim (a fresh id is
                                      minted); {history} is the flat compat
                                      form [{role, content, createdAt,
                                      toolName?, usage?, …}]
POST   /api/pair                      { code } → { token } — unauthenticated
                                      bootstrap; the one-time code authorizes
                                      it (see Auth)
POST   /api/auth/login                { token } → 200 + httpOnly
                                      sepia_token cookie (Secure over https)
POST   /api/auth/logout               expires the cookie → { ok: true }
GET    /api/events                    node event feed SSE (see below)
POST   /api/client/keypair            mint a client-identity keypair —
                                      { algorithm: "Ed25519", publicKey,
                                      secretKey } (base64url) — for clients
                                      on non-secure contexts where
                                      crypto.subtle is unavailable; the
                                      secret transits the wire
GET    /api/projects                  { projects } — node-local projects
POST   /api/projects                  { name } → 201 { project }
PATCH  /api/projects/:id              { name } — rename → { ok: true }
                                      (404 on unknown id)
DELETE /api/projects/:id              → { ok: true }
GET    /api/config                    { config } — server-side UI state
PATCH  /api/config/:key               { value } — stored verbatim →
                                      { key, value }; keys "vapid" and
                                      "pushSubscriptions" are internal:
                                      hidden from GET and refused (400)
GET    /api/push/vapid                { publicKey } — the VAPID key clients
                                      subscribe with
POST   /api/push/subscribe            { endpoint, keys: { auth, p256dh },
                                      prefs? } → { ok: true }
DELETE /api/push/subscribe            { endpoint } → { ok: true }
```

Meta-dependent routes (`PATCH /api/sessions/:id`, `/api/projects`,
`/api/config`) answer **501** when the node runs without a meta store;
convert/import, pairing and push likewise 501 when their backing service is
unconfigured.

## The session stream

`GET /api/sessions/:id/stream` — SSE of `sepia-proto::SessionEvent` values,
one `data: <json>` frame per event (no `event:` name — the JSON's `type`
field discriminates). Keep-alive rides as `: ping` comments on the
`SEPIA_SSE_KEEPALIVE_MS` cadence (`0` disables); a `event: lagged` frame
marks a subscriber that fell behind the broadcast ring (clients refetch on
resync anyway).

Event `type` values (`camelCase` fields):

- `runStarted` / `runFinished` — `{threadId, runId}` run lifecycle edges.
- `textMessageStart` `{messageId, role}`, `textMessageContent`
  `{messageId, delta}`, `textMessageEnd` `{messageId}` — assistant/user
  text frames.
- `reasoningMessageStart|Content|End` — same shape, thinking frames.
- `toolCallStart` `{toolCallId, toolCallName, locations?, diffs?,
  contents?}`, `toolCallArgs` `{toolCallId, delta, toolCallName?}`,
  `toolCallResult` `{messageId, toolCallId, content}`, `toolCallEnd`
  `{toolCallId, status?, toolCallName?, locations?, diffs?, contents?}` —
  tool-call lifecycle.
- `custom` `{name, value}` — escape hatch for ACP-specific payloads
  (`acp:plan`, `acp:permission_request`, mid-call file updates, unknown
  update kinds).

## The node event feed

`GET /api/events` — one SSE stream per node, the aggregated-UI replacement
for polling. Frames are `event: <kind>` + `data: <json>`:

```
event: session     data: {"id":"...","agent":"...","patch":{"busy":true}}
event: meta        data: {"id":"...","agent":"...","patch":{"pinned":false}}
event: project     data: {"id":"...","patch":{"name":"Web"}}
event: heartbeat   data: {"ts":1700000000}
```

- `session` — a summary row changed (created, updated, deleted, `busy`
  flips, `runFinished`/`permissionRequested` markers). While a session is
  held by another process (a read-only attach), the node's held-session
  watch re-probes it every `SEPIA_HELD_WATCH_MS` and emits
  `locked`/`lockHolderPid`/`updatedAt` diffs — the lock-release edge and
  the holder's transcript flushes arrive here instead of clients polling
  `GET /api/sessions?withLocks=1`.
- `meta` — the overlay (title/pinned/archived/projectIds/model/spans)
  changed, same `{id, agent?, patch}` shape.
- `project` — a project row changed (`patch.name`, or `{deleted: true}`).
- `heartbeat` — keepalive every `SEPIA_SSE_KEEPALIVE_MS`.

The UI invalidates the matching queries on receipt. Nodes with no clients
still run the control plane; events are best-effort, so a missed event just
means a slightly stale row until the next refetch.

## Projects

Node-local: `{ id, name, cwd }` where `cwd` is a path on _that_ node. The
aggregated UI shows `name @ node` (or a machine badge). Cross-machine
grouping of like-named projects is a UI concern — no sync, and no
bundle/transfer routes in this implementation.

## Auth — bearer, cookies, pairing

One credential model underneath:

- **`SEPIA_TOKEN`** — set on the node; every `/api/*` route requires
  `Authorization: Bearer <token>` (401 + `WWW-Authenticate: Bearer`
  otherwise). An unset token means auth is off entirely. Non-loopback binds
  (`SEPIA_HOST` not 127.x/localhost/::1) refuse to boot without it.
- **`sepia_token` cookie** — `POST /api/auth/login { token }` exchanges a
  presented credential for an httpOnly `SameSite=Strict` cookie; Bearer wins
  when both ride. `POST /api/auth/logout` expires it.
- **`?access_token=` is SSE-only.** EventSource can't set headers, so the
  query credential is honored on just GET `/api/events` and GET
  `/api/sessions/:id/stream`. Everywhere else needs the Bearer header (or
  cookie) — the token never lands in a URL where it's avoidable.

**Pairing** — `sepia pair` (run on the node) writes `{code, expiresAt}` to
`$SEPIA_HOME/pair-code`. `POST /api/pair { code }` redeems it: codes are
Crockford base32 in a 4-4 group (`7K2M-9PQX`), case-insensitive, ~60s TTL,
single-use. Unknown/expired/used all answer the same 404
`{error: "Invalid or expired pairing code"}` — the response doesn't leak
which case. Success returns `{ token: "sepia_…" }`, a bearer credential
equivalent to `SEPIA_TOKEN`; issued credentials persist as sha256 hashes in
`$SEPIA_HOME/tokens.json` — deleting that file revokes them.

## Errors

Failures are `{ "error": "<message>", "code": "<snake_case>"? }`:

- `not_found` → 404 (also the generic fallthrough `{error:"Not found"}`)
- `invalid`, `unknown_agent` → 400 (plain validation 400s carry no `code`)
- `locked`, `conflict`, `busy` → 409
- `internal` → 500
- unconfigured features (meta store, convert/import, pairing, push) → 501
  `{error: "<Feature> is not configured on this server"}`
- unauthorized → 401 `{error: "Unauthorized"}`

## Environment

Boot-time (`crates/sepia-http/src/env.rs`, `crates/sepia-node`):

- `SEPIA_HOST` (default `127.0.0.1`), `SEPIA_PORT` (or `PORT`; default 8787).
- `SEPIA_TOKEN` — bearer credential; required off-loopback.
- `SEPIA_HOME` (default `~/.local/share/sepia`) — node.json, meta.json,
  tokens.json, pair-code.
- `SEPIA_NODE` — node identity path (default `$SEPIA_HOME/node.json`);
  `SEPIA_NAME` — display name (default: hostname).
- `SEPIA_META`/`SEPIA_META_PATH` — meta overlay JSON.
- `SEPIA_ORIGINS` — comma-separated CORS allowlist (`*` echoes any origin;
  bearer auth, not CORS, is the gate). `Vary: Origin` rides every response.
- `SEPIA_CLINE_DIR` (default `~/.cline/data`) — convert/import target;
  `SEPIA_CLAUDE_DIR` (default `~/.claude`) — file-history restore root.
- `SEPIA_SSE_KEEPALIVE_MS`, `SEPIA_HELD_WATCH_MS` — SSE cadence and the
  held-session re-probe (`0` disables each).
- `SEPIA_IDLE_TTL_MS`, `SEPIA_SWEEP_MS` — idle live-session detach tuning.
- `SEPIA_AGENT_<ID>_COMMAND` — substitute the agent argv a driver spawns.

Read directly by the control plane / ACP layer (not the `Env` struct):
`SEPIA_LOCK_TTL_MS`, `SEPIA_HISTORY_LIMIT`, `SEPIA_INHERIT_ENV`,
`SEPIA_DEBUG`. Store paths belong to the drivers (`SEPIA_DEVIN_DB`, …) —
the node never opens them itself.

## Safety boundary

The API mutates _sessions_, not the machine: create/attach/prompt/cancel/
patch/delete on agent stores, plus meta overlay and projects. No shell, no
filesystem writes outside the stores (restore writes only recorded files
under the session's cwd), no arbitrary process control — that contract is
part of the protocol.

## Failure model

- A node that's down just doesn't appear — lists render from the nodes that
  answer. Timeouts are short so one dead laptop doesn't stall the list.
- `/api/events` reconnects with backoff; a missed event (or a `lagged`
  stream frame) is a stale row, not lost data — refetch to resync.
- Optimistic mutations roll back per node.
