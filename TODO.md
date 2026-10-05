# TODO

## Shipped

Federation (node identity, peers, events, pair, binary, spans, resume,
gateway + HTTPS upstreams), IR v2 (usage, tool status, lineage, model,
blocks, thinking signatures, checkpoints+diffs), session lifecycle
(file restore + rewind per store), 4 adapters (devin, cline, claude,
cursor — all read+write), details tabs, sub-agents, sidebar config,
mobile polish, coverage, takeover, folder-by-node, live contents.

## Remaining

### Deferred by design

- [ ] `sessionCapabilities`/`promptCapabilities` probing — attachments
      are sent blind; a peer's advertised capability isn't read
- [x] `sepia` CLI `import/export`/`install`/`list`/`delete` for
      claude/cursor — `--from`/`--to` + `--claude-dir`/`--cursor-dir`;
      `ClaudeCode.toJsonl` writer added (the store was read-only)
- [ ] `lockHolderPid` across agents — only the default agent's
      `session/list` is probed; a cline-held session's holder is
      invisible to the devin probe
- [ ] Mid-attach replay — attaching mid-tool-call drops args (no
      TOOL_CALL_START → the row gets no accumulated args)
- [x] Devin `prompt_history`/`rendered_commits` on rewind — investigated:
      no reliable join to `message_nodes` exists for either table, so they
      stay; rationale documented on `SqliteStorage.truncateSessionNodes`

### Housekeeping

- [x] `AGENTS.md` stale claims (sqlite stub note, coverage policy) — stubs
      live in `apps/server`/`session-control` vitest configs; thresholds are
      per-file floors, not a blanket 100%
- [x] `package.json` script name vs `vp` built-in drift (`vp dev` vs
      `vp run dev`) — `vp run dev` is canonical: root script runs
      `sepia-web#dev` → `vp dev` inside `apps/web`; bare `vp dev` at the
      workspace root errors (needs a package target)
- [ ] Push-notification end-to-end verification on a real device —
      subscribe/reconcile/test coverage exist; real push untested
