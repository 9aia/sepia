# TODO — Rust rewrite

## Open

### Security

- [ ] **Browser→hub auth surface** — the hub currently trusts the local
      caller (127.0.0.1 default). Exposing the hub off-loopback needs its
      own auth layer (token or OIDC front) — node bearer tokens stay
      server-side, so no credential store exists client-side.
- [ ] **Pair rate limiting** — `POST /api/pair` has none. Codes are
      ~40 bits/60s so online brute force is infeasible, but add a per-IP
      throttle if a node is ever exposed off-LAN without a proxy.
- [ ] **Hub `SEPIA_NODES` token storage** — node tokens live in
      `$SEPIA_HOME/hub/` config env today; keyring-backed storage is the
      upgrade path.

### Ops / QA

- [ ] **Push E2E on a real device** — subscribe/fan-out paths are unit-
      covered; a real browser push is unverified.
- [ ] **Browser e2e** — no WebDriver suite yet (thirtyfour vs playwright
      decision deferred; hub↔node SSR e2e covers the critical path).
- [ ] **Unported surfaces** — managed-server registry, SSH tunnels,
      gateway proxy (`/api/servers`, `/api/gateway`), project transfer,
      OTEL. Intentionally out of scope for v0 — nodes must be directly
      reachable (`SEPIA_NODES` URLs); revisit when tailscale-less remote
      nodes matter.
- [ ] **`cargo leptos` wasm bundle in `xtask install`** — hydration JS
      (`pkg/sepia_web.js`) requires a cargo-leptos build; `xtask install`
      ships SSR-only today (UI works hydrated-only-after `cargo leptos
      build`).
- [ ] **macOS verification** — all e2e ran on Linux; devin/cline path
      defaults and service install need a mac pass.
- [ ] **Body-cap audit** — large JSON handlers are capped; sweep for any
      remaining unbounded body read in `sepia-http`.

## Done (Rust)

Whole-tree rewrite of the TS system: core IR + ports, four store drivers
as discovered binaries, tolerant ACP client, control plane (attach/
takeover/locks/idle sweep), axum REST+SSE, headless node, full CLI,
durable outbox + sync projection (subscribe-before-list, per-session
FIFO), Leptos SSR+hydrate UI with session ops, PWA shell + hub-owned
push fan-out, fault-injection + soak suites, real-subprocess e2e
(node↔driver↔agent↔hub). Credential model fixed by architecture: node
bearer tokens live in the hub config, never in the browser.
