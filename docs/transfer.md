# Cross-node session + project transfer

"Resume on a different machine" — the hub moves a session (or every
session under a working directory — a cwd "project") from one paired
node to another, so the agent's `resume` continues on the target.

## Protocol

The hub orchestrates; nodes stay dumb store endpoints. Per session:

1. **Resolve** — the hub's sync projection maps the session id to its
   owning node (`session_index.node_id`). `?agent=` disambiguates ids
   that collide across stores on the same node. A same-node transfer
   (`source == target`) is rejected — that's what `/convert` is for.
2. **Pre-flight** — `GET /api/node` on the target. A descriptor whose
   `capabilities` lacks `"import"` fails fast with a clear error
   before any export bytes move (the import route 501s the same way —
   the pre-flight just makes the failure legible and cheap).
3. **Export** — `GET /api/sessions/{id}/export?agent=` on the source
   returns `{session: {…IR…}}`, the complete session IR: messages,
   content blocks, checkpoints metadata, cogs, model.
4. **Import** — `POST /api/sessions/import` on the target with
   `{agent, session, cwd?, title?, model?}`:
   - `agent` — the target store. The session's own backend when it's a
     store-write target (`"cline"`/`"devin"`), else `"devin"` (today's
     store-write seam only lands in the devin store regardless — see
     Limits).
   - `cwd` — only when remapping; absent, the IR's
     `working_directory` rides through verbatim.
   - `title`/`model` — the summary's overlay values, so a renamed or
     model-pinned session looks the same after the move.
   - The import route **always mints a new session id** server-side —
     transfers are copies, never moves.
5. **Meta copy** — `PATCH /api/sessions/{new_id}` on the target with
   the source's `pinned`/`archived` flags. Best-effort: a failure is
   reported in the result's `warnings`, not a failed transfer.
6. **Verify** — `GET /api/sessions/{new_id}` on the target. A missing
   row downgrades to a warning (the 201 already vouched for the
   write); the hub's sync feed surfaces the copy on its own cadence.

`transfer_project(cwd, source_node, target_node, cwd?)` runs the same
pipeline per session: every projected session on `source_node` whose
`cwd` matches. The optional `cwd` remaps **all** of them (for
`/home/laptop/proj` → `/home/desk/proj` moves). Sessions are
transferred sequentially and independently — one failure doesn't stop
the rest; the result carries `transferred` and `failed` lists.

## What carries

- Full session IR — transcript, tool-call blocks, token usage, model,
  backend type, parent-session links.
- Meta overlay, partially: `title` and `model` ride the import body;
  `pinned`/`archived` ride the follow-up meta PATCH.
- Run provenance: the target node's import handler appends a
  `{at, agent, node}` run span for the new session — the same record
  an attach writes — so the copy's history shows it was born here.

## What doesn't

- **The working directory.** Transfer moves the *session*, not the
  checkout. Resuming on the target requires the project's files to
  exist there — clone/sync them yourself, and pass `cwd` when the path
  differs (`/home/laptop/proj` → `/home/desk/proj`). Nothing validates
  the directory on the target.
- **Checkpoint snapshot data.** Workspace snapshots (file-history
  blobs a `restore`/`rewind` reads) live on the source machine. The
  checkpoint refs ride the IR, but rewinding on the target can only
  reach transcript-level cuts, not filesystem restores.
- **`projectIds`.** Named projects are per-node meta registries; an id
  from node A names nothing on node B. Re-add the copy to a project on
  the target.
- **Live state** — locks, attaches, in-flight runs. Transfer an idle
  session; a `busy` source row keeps running on the source.
- **Pending outbox writes** queued for the source stay queued for the
  source.
- **The source copy.** Transfer is a copy — the original session keeps
  living on the source node. Delete it there if the move should be
  final. Re-transferring the same session makes another copy (imports
  mint fresh ids; there's no dedupe).

## Conflicts

None by construction: the target import always mints a new session id,
so "same session id already on target" can't happen. Repeating a
transfer creates additional independent copies. Session ids *can*
collide across stores on one node — `agent` disambiguates at export
exactly like the rest of the session API.

## Partial failure

- `transfer_session` is all-or-error per session: export, pre-flight,
  or import failure → `Err`. Meta-copy/verify failures → `Ok` with
  `warnings`.
- `transfer_project` never aborts on one session's failure — it
  returns `{transferred: [...], failed: [{sessionId, title, error}]}`
  so the UI can report "moved 4 of 5".

## Limits

- Store-write requires a node's convert/import seam — today that's the
  devin store (`sepia-node` wires it when a `devin` store is
  registered; other drivers have no store-write path). The node
  advertises `"import"` in `/api/node` capabilities when the seam
  exists; without it the transfer fails the pre-flight ("target node
  does not support session import") rather than a bare 501.
- `agent` other than `cline`/`devin` in the import body is a 400 —
  sessions from other backends land in the devin store (their
  `backend_type` is preserved in the IR).
- Single-node hubs can't transfer (nowhere to send). The UI hides the
  affordance; the API errors plainly.
