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

### Security (audit 2025 — fixed items live in docs/protocol.md "Hardening notes")

- [ ] **Client credential store encryption at rest** (`sepia:credentials`,
      `sepia:token` in localStorage). Threat model: plaintext today defends
      only against nothing — any same-origin script or anyone who can read
      the browser profile recovers every peer token. Options weighed:
      (a) AES-GCM keyed from `sepia:client.secretKey` — pure obfuscation,
      the key sits in the same localStorage, so we did NOT ship it;
      (b) non-extractable AES-GCM CryptoKey in IndexedDB — genuinely better
      on Chrome/macOS+Windows where OSCrypt wraps IDB keys with the OS
      keychain (defeats profile-directory theft), but Linux falls back to a
      fixed OSCrypt password AND the sync stores (`getToken()`,
      `peerSecret()`) would need async hydration with a boot race (401
      flash, transiently credential-less peers); (c) document + defer —
      chosen. Real fix is either (b) behind a hydration gate or a
      server-issued httpOnly cookie + OS keychain for peer creds.
- [ ] **Rate limiting on `POST /api/pair`** — none today. Codes are ~40 bits
      and 60s-lived, so online brute force needs ~10¹⁰ req/s — infeasible —
      but there is no per-IP throttle; add one if the node is ever exposed
      off-LAN without a reverse proxy (deploy behind Caddy/nginx limits).
- [ ] **Residual gateway SSRF** — `/api/servers` accepts any non-denied
      host by design (loopback/private ARE legitimate managed nodes), and
      the proxy injects the entry's stored credential at its
      `scheme://host:port`. Denylisted: unspecified + link-local/metadata
      (checked on the normalized hostname, so odd IP spellings can't
      slip). Still open: a _public_ hostname whose DNS resolves to a
      denied/private address at fetch time (rebinding — a per-request
      resolver check is the fix), and an authenticated caller can always
      point an entry at an arbitrary internal HTTP service; the bearer
      already grants agent-level code exec, so this is bounded, but worth
      revisiting if scoped tokens ever ship.
- [ ] **`SEPIA_ORIGINS=*` echoes any Origin** — deliberate (federated UIs on
      other machines), and bearer auth remains the gate since no
      ambient credential (cookie) exists. Note the interaction: a
      `?access_token` URL pasted into a cross-origin page is replayable —
      keep the SSE-only restriction in place.
- [ ] **httpOnly cookie transport for the local token** — would remove the
      token from JS reach (XSS) and from SSE URLs entirely; needs SameSite + CORS credential plumbing and a CSRF story, so deferred until
      session-style auth is worth it.

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
