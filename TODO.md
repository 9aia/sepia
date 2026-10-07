# TODO

## Open

### Security (deferred — rationale also in docs/protocol.md "Hardening notes")

- [ ] **Client credential store encryption at rest** (`sepia:credentials`,
      `sepia:token` in localStorage). Plaintext today: any same-origin
      script or anyone who can read the browser profile recovers every
      peer token. Options weighed: (a) AES-GCM keyed from
      `sepia:client.secretKey` — pure obfuscation (key sits in the same
      localStorage), not shipped; (b) non-extractable AES-GCM CryptoKey in
      IndexedDB — genuinely better on Chrome/macOS+Windows (OSCrypt wraps
      IDB keys with the OS keychain) but Linux falls back to a fixed
      OSCrypt password AND the sync stores (`getToken()`, `peerSecret()`)
      need async hydration with a boot race (401 flash); (c) document +
      defer — chosen. Real fix: (b) behind a hydration gate, or a
      server-issued httpOnly cookie + OS keychain for peer creds.
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
- [ ] **Rate limiting on `POST /api/pair`** — none today. Codes are ~40
      bits and 60s-lived, so online brute force needs ~10¹⁰ req/s —
      infeasible — but there is no per-IP throttle; add one if the node is
      ever exposed off-LAN without a reverse proxy (deploy behind
      Caddy/nginx limits).
- [ ] **httpOnly cookie transport for the local token** — would remove the
      token from JS reach (XSS) and from SSE URLs entirely; needs
      SameSite + CORS credential plumbing and a CSRF story, so deferred
      until session-style auth is worth it.

### Ops / QA

- [ ] Push-notification end-to-end verification on a real device —
      subscribe/reconcile/test coverage exist; real push untested
- [ ] `SEPIA_*` body-caps audit — the large JSON handlers are capped
      (`/api/servers`, proxy path, prompt parts); sweep for any remaining
      unbounded `request.json()`/`arrayBuffer()` route

## Done (previously "Shipped" + "Deferred by design")

Architecture: ports-and-adapters split (`sepia-core` pure domain+ports;
`sepia-{devin,cline,claude,cursor}` isolated adapters; `sepia-convert`;
control plane on ports; server routes decomposed); perf (SQL-paged
history, mtime-stamped list caches, pooled ACP lock probes, bounded
callsIndex); `bun --compile` single binary (embedded UI, default deploy
path); release tooling (`vp run release` → stamp → builds → tag/push →
`bun publish` → `gh release`); ReasoningBlock test-timeout flake fixed.

Federation: node identity, peers, events, pair, binary, spans, resume,
gateway + HTTPS upstreams, hardened proxy (timeouts, body caps,
credential confinement, origin pinning), project transfer + single-
session pull/push, capability probing + gating (prompt parts, attach,
delete — advertised capabilities surface in details), `lockHolderPid`
across agents, mid-attach replay (lazy tool-call rows).

IR v2 (usage, tool status, lineage, model, blocks, thinking signatures,
checkpoints+diffs), session lifecycle (file restore + rewind per store —
Devin `prompt_history`/`rendered_commits` kept on rewind: no reliable
join; rationale on `truncateSessionNodes`), 4 adapters all read+write,
CLI `import/export/install/list/delete` for claude/cursor.

UI: details tabs, sub-agents, sidebar config, mobile polish, takeover
flows, folder-by-node, live contents, client identity (label + keypair
incl. server-mint fallback for insecure contexts), Desktop {node, agent,
model, cwd} + footer picker, federated catalog (Models/Agents/Nodes +
per-item toggles), credential store, skeleton/hydration honesty across
the sidebar + chat header, focus-ring consistency, context-only-at-
absolute-start.

Security hardening shipped: address-bound tokens, SSE-only query auth,
gateway confinement to `/api/*` + origin pinning, SSH argv guards,
capability gating, auth-gated keypair mint, server body caps. (Deliberate
`SEPIA_ORIGINS=*` behavior lives in docs/protocol.md "Hardening notes".)

Housekeeping: AGENTS.md layout + stubs + coverage-policy notes corrected;
`vp run dev` canonicalized; per-package coverage floors recalibrated
post-split.
