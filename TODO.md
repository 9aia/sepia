# TODO

## Federation (docs/protocol.md)

- [x] `GET /api/node` — identity + capabilities
- [x] Node registry + merged lists keyed `node:agent:id` + machine badges
- [x] `GET /api/events` — node SSE feed
- [x] `sepia pair` — code → credential; `POST /api/pair` filesystem-gated
- [x] `bun --compile` binary — `sepia serve` serves API + embedded SPA
- [x] `meta.spans` — per-run agent/node provenance + transcript markers
- [x] Resume anywhere — `POST /import` + `GET /export` (full-IR, flat fallback)
- [ ] Gateway/proxy mode for unreachable peers (phase 3 — defer)

## Session IR (docs/session-formats.md)

- [x] IR v2 — usage/cost, tool status + exitCode/durationMs, parent/agent
      lineage, model/requestId/finishReason — read + write + wire + render
- [x] Content blocks — image/audio/file attachments survive the IR
- [ ] Thinking signatures — replay-safe signed thinking (Devin `signature`,
      Claude `redacted_thinking`); currently text-only
- [ ] Checkpoints/file-diff refs — Cline checkpoint metadata, Devin
      tool-call file diffs
- [ ] Sub-agent children in session details (we have `parentSessionId`;
      list "Sub-agents" rows in the drawer)
- [ ] Prompt attachments — `POST /prompt` is text-only today; widening
      PromptPart needs the ACP call shape

## New adapters (docs/session-formats.md has the layouts)

- [ ] Claude Code — `~/.claude/projects/*/<session>.jsonl`
- [ ] Cursor — `~/.cursor/chats/<ws>/<chat>/store.db` blobs + global
      `state.vscdb` `cursorDiskKV` bubbles (no per-session store)

## Housekeeping

- [ ] sepia-core 100% coverage — `bun:sqlite` modules can't run under Node
      vitest; needs a Bun-side coverage run or threshold split
- [ ] `ai/` dep on apps/web — still pulled by `prompt-input.tsx` types;
      check a lighter type-only path
- [x] `todo/` root-owned dir — removed

## Deferred notes

- Old `import` targets receiving `{session}` 400 — flat-history path only
  works old→new, not new→old (`docs/protocol.md` notes it).
- `sepia-server` `serve.ts` throws instead of `process.exit` (clean for the
  binary wrapper; callers must handle).
