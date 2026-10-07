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
POST   /api/sessions/:id/prompt       send { text?, attachments? } — attachments
                                        is an ACP content-block array (image,
                                        audio, resource, resource_link)
POST   /api/sessions/:id/cancel       stop the run
POST   /api/sessions/:id/permission   reply to a pending permission
GET    /api/sessions/:id/checkpoints  { checkpoints } — workspace snapshot
                                      refs the store recorded (Cline shadow-git)
POST   /api/sessions/:id/restore      file restore — writes under the session's
                                      cwd; requires { confirm: true }:
                                      { path, toolCallId? } reverts the file via
                                        the recorded diffs (pre-session state,
                                        or just that call's change)
                                      { checkpoint, paths? } materializes the
                                        files a checkpoint ref covers
                                      refused while the session is busy or
                                      locked by a live process; per-file
                                      { restored, skipped } report
POST   /api/sessions/:id/rewind       conversation rewind — truncates the
                                      transcript, not files (that's restore).
                                      Requires { confirm: true } plus exactly
                                      one selector: { nodeId } keeps that node
                                      and everything before it (a history row's
                                      nodeId), { turns } drops the last N user
                                      turns, { checkpoint } rewinds to a
                                      recorded snapshot ref. Same gates as
                                      restore; a live attach is detached
                                      first. → { kept, removed }; 409 for a
                                      store that can't truncate safely
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
GET    /api/projects/:id/export       the project bundle — a streamed
                                      application/x-ndjson document (see
                                      "Project transfer")
POST   /api/projects/import           consumes an NDJSON bundle body (parsed
                                      streaming, so big projects don't hit
                                      the JSON body's size ceiling); writes
                                      sessions through the same store paths
                                      as /api/sessions/import and re-points
                                      the meta overlay at the local project
POST   /api/projects/pull             { source: { url, token? }, project } —
                                      this node fetches the source's export
                                      with the supplied credential and
                                      imports it (node-to-node). Response is
                                      SSE: start, session×N, done | error
POST   /api/projects/:id/push         { target: { url, token? } } — this node
                                      bundles the project and POSTs it to
                                      target.url's /api/projects/import with
                                      target.token. Same SSE progress shape
POST   /api/client/keypair           mint a client-identity keypair
                                        ({ algorithm, publicKey, secretKey })
                                        server-side — for clients on
                                        non-secure contexts (http:// LAN)
                                        where crypto.subtle is unavailable;
                                        the secret transits the wire, so it's
                                        only as private as the transport
GET    /api/config/:key  PATCH /api/config/:key   server-side UI state
GET    /api/push/vapid  POST/DELETE /api/push/subscribe   web-push
ANY    /api/gateway/:server/*           gateway mode — forward to a managed
                                        server registry entry (the same store
                                        as /api/servers) at its stored
                                        scheme://host:port (TLS upstreams
                                        included) with its stored
                                        credential injected; the caller's own
                                        token (incl. ?access_token) is
                                        consumed by the node and never
                                        forwarded. Only /api/* paths on the
                                        upstream origin forward (checked on
                                        the normalized URL — `..` escapes
                                        and non-API paths are refused).
                                        SSE-safe: the client disconnect
                                        cancels upstream.
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

- `session` — a summary row changed (created, updated, deleted). While a
  session is held by another process (a read-only attach), the node's
  held-session watch re-probes it every `SEPIA_HELD_WATCH_MS` and emits
  `locked`/`lockHolderPid`/`updatedAt` diffs — the lock-release edge and the
  holder's transcript flushes arrive here instead of clients polling
  `GET /api/sessions?withLocks=1`.
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

### Project transfer

Projects are the git-like unit of movement between nodes — sessions decouple
from the machine the agent runs on, so a project moves wholesale. The verbs:

- `GET /api/projects/:id/export` — streams the bundle:
  `application/x-ndjson`, one JSON object per line. The first line is
  `{"type":"project","version":1,"id","name","node":{"id","name"},"sessions":N}`;
  then one `{"type":"session","id","agent","title","meta","session"}` per
  member — `session` is the full IR from `GET /api/sessions/:id/export`
  (nodes, toolCalls, thinking, usage, checkpoints verbatim) and `meta` is the
  sepia overlay minus `projectIds` (node-local). A member the store can't
  read becomes `{"type":"skipped","id","error"}`; the trailer is
  `{"type":"end","sessions":N,"skipped":M}`. Unknown line types are ignored.
- `POST /api/projects/import` — consumes a bundle body, line by line (the
  parse streams, so project size is bounded by per-line memory, not a body
  cap). The `project` line creates the project under its source id — or
  refreshes the name when it already exists, which is what makes a re-pull
  an update rather than a clone. Each session writes through the
  `/api/sessions/import` executor: `cline` members land in the Cline store,
  everything else in the Devin store; the overlay then re-points
  `projectIds` at the local project, restores title/pinned/archived/model/
  spans, and appends a run span for this node. `meta`/`session`/`project`
  feed events fire as rows land. → `201 { project, imported, skipped,
truncated }`.
- `POST /api/projects/pull` — `{ source: { url, token? }, project }` — the
  receiving node fetches the source's `export` itself (no browser relay)
  and imports it. The response streams progress as SSE: `start`, one
  `session` frame per landed session (`{index, total, id, sourceId, agent,
title}`), then `done` (the import summary) or `error`.
- `POST /api/projects/:id/push` — `{ target: { url, token? } }` — the owning
  node POSTs the bundle to `target.url`'s `/api/projects/import`
  authenticated with `target.token`. Same SSE shape.

Clone is pull-with-a-new-id — the same op covers both cases since import is
idempotent by id. `init` is just `POST /api/projects` (`sepia projects
init`). Auth is the existing bearer model: the client authenticates to the
node it calls, and hands the peer's credential (`source.token` /
`target.token`) to the node for the cross-node leg — a `via: "gateway"`
peer resolves to this node's `/api/gateway/<serverId>` mount authenticated
with the local token, so unreachable peers transfer through the same path
the rest of federation uses.

`sepia projects export|import|pull|push` drive the same endpoints from the
CLI.

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

Credentials live in the UI's credential store (`localStorage`; peers link
one by `credentialId` — OS keychain later). CORS allows the serving origin +
any registered peer origins.

Hardening notes:

- **`?access_token` is SSE-only.** EventSource can't set headers, so the
  query credential is accepted only on GET `/api/events` and GET
  `/api/sessions/:id/stream` (including under the gateway/proxy mounts).
  Every other endpoint requires `Authorization: Bearer` — a token can never
  authenticate its way into a URL on a path where it's avoidable, which
  keeps it out of browser history and proxy logs there. The server access
  log records only `url.pathname`, never the query.
- **Client tokens are origin-bound.** `sepia:token` in localStorage is a map
  of `{node address → token}` — a token entered for one node is never sent
  to another, so repointing `settings.localNodeUrl` at a different machine
  can't exfiltrate the serving origin's credential (the new node just 401s
  and the gate re-prompts for _its_ token).
- **The client keypair is not a store key.** `sepia:client`'s `secretKey`
  lives in the same localStorage as the values it could encrypt — using it
  as an encryption root would be obfuscation, not protection, against anyone
  who can read the profile. The credential store is deliberately plaintext
  until a scheme with a real key boundary (OS keychain, or a non-extractable
  IndexedDB key + async hydration) lands — see TODO.md.
- **Push endpoints are authenticated.** `GET /api/push/vapid` and
  `POST/DELETE /api/push/subscribe` require the bearer like everything else
  — an open subscribe would let a network peer register its own endpoint
  and receive notification payloads.
- **Managed-server hosts are validated.** Registry entries accept only
  hostname/IP literals (no `@`, `:`, `%`, whitespace — nothing that smuggles
  URL syntax or ssh argv), and the URL-normalized host is denied if it's
  unspecified (`0.0.0.0`, `::`) or link-local (incl. `169.254.169.254` and
  `metadata.google.internal`). Loopback/private stays legal — managed nodes
  legitimately live there; the residual SSRF shape is in TODO.md.
- **`ssh.user`/`ssh.host` are login-name/hostname charsets.** Both land in
  the `ssh` argv (`user@host`, `-L` spec); a leading `-` or embedded
  punctuation would parse as option flags.

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
3. **Phase 3** — gateway mode: one node proxies unreachable peers (a peer
   behind NAT, a client on a phone on another network, CORS- or
   auth-complicated upstreams). A peer in the UI registry marked
   `via: "gateway"` resolves to `ApiTarget{baseUrl: "/api/gateway/<id>"}`
   instead of its own origin, so every call — merged lists, session actions,
   `/stream` + `/events` SSE — rides the node's forward unchanged. The
   gateway id is a managed-server registry entry (`/api/servers`), which is
   where the peer's url + credential live: the UI registers gateway peers
   server-side, and a credential submitted at add-time never persists on the
   client. The peer sees only its own stored token — the key-translation
   layer this phase adds — including for EventSource's `?access_token` query
   auth, which the proxy strips before forwarding.
