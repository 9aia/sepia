# TODO

## All shipped

Federation (node identity, peers, events, pair, binary, spans, resume,
gateway), IR v2 (usage, tool status, lineage, model, blocks, thinking
signatures, checkpoints+diffs, file restore), 4 adapters (devin, cline,
claude, cursor), session-details tabs (Reports/Prompt/Rules/Skills),
sub-agent tree, sidebar config, mobile polish, coverage, takeover,
folder-by-node, real takeover errors.

## Remaining

### Worth doing

- [ ] Keybinds coverage — matching layer under-tested
      (useAppHotkey/useGlobalHotkey matching, overrides, conflicts)
- [ ] `store.db` write path for Cursor — save/delete goes to the
      agent-transcripts projection only; canonical store.db stays read-only
      (resumed cursor sessions won't open in Cursor itself)
- [x] HTTPS gateway upstreams — managed registry stores scheme/host/port,
      `https://` peers keep TLS through the proxy + gateway mounts
- [ ] Live `terminal`/`content` ToolCallContent — normalize drops non-diff
      content (terminal output refs never reach live rows)
- [ ] Session rewind — restore is file-level only; no IR deletion model
      for truncating a conversation to a checkpoint

### Deferred by design

- [ ] Gateway: `sessionCapabilities`/`promptCapabilities` read — we never
      probe what a peer's agent advertises; attachments are sent blind
- [ ] `sepia` CLI `import/export`/`install` for claude/cursor — adapters
      have the IR but no CLI verbs wired
- [ ] `lockHolderPid` across agents — only the default agent's
      `session/list` is probed; a cline-held session's holder is invisible
      to the devin probe
- [ ] Mid-attach replay — attaching mid-tool-call drops args (no
      TOOL_CALL_START → the row gets no accumulated args)

### Housekeeping

- [ ] `AGENTS.md` stale claims (sqlite stub note, coverage policy)
- [ ] `package.json` script name vs `vp` built-in drift (`vp dev` vs
      `vp run dev`)
