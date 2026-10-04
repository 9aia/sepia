# TODO

## Multi-node / federation — `docs/protocol.md`

- [ ] `GET /api/node` — `{ id, name, version, protocol, agents, capabilities }`,
      stable id in `$SEPIA_HOME/node.json`, `name` defaults to hostname
- [ ] UI node registry — `{ url, token, name }` in settings; node picker UI
- [ ] Merged session/project lists keyed `node:agent:id`, machine badges,
      actions routed to the owning node
- [ ] `GET /api/events` — node SSE feed (session/meta/project/heartbeat),
      replaces per-node polling
- [ ] `sepia pair` — short-code → long-lived credential exchange
- [ ] `bun --compile` single binary serving API + built UI
- [ ] Gateway/proxy mode for unreachable peers (phase 3)

## Web app

- [ ] `tests/e2e/reply.spec.ts` — flaky sent-row assertion (optimistic→history
      handoff timing); the prompt lands but the row check is racy
- [ ] Reasoning/tool-call polish — streaming state parity with the old
      ai-elements behavior
- [ ] `vp run -r build` for apps/web — production bundle check + SW registration
      in the built output
- [ ] Mobile pass — dialogs/sheet widths, drawer height, toast stacking on
      small screens

## Server

- [ ] `POST /api/pair` — one-time-code → credential (protocol doc)
- [ ] `/api/events` — the node event feed
- [ ] `SEPIA_HOME` — node.json + meta + push store consolidation
- [ ] Gateway proxying (defer)

## Housekeeping

- [ ] `node_modules`/`dist` leftovers in `todo/` (root-owned dir) — needs
      `sudo rm -rf ~/GitHub/9aia/sepia/todo`
- [ ] `ai/` dep on apps/web — still needed by `prompt-input.tsx` types;
      check if a lighter type-only path exists
