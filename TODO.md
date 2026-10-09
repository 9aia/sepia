# TODO — Rust rewrite

## Open

### Security

- [ ] **Pair rate limiting** — `POST /api/pair` has none. Codes are
      ~40 bits/60s so online brute force is infeasible, but add a per-IP
      throttle if a node is ever exposed off-LAN without a proxy.
- [ ] **Hub `SEPIA_NODES` token storage** — node tokens live in
      `$SEPIA_HOME/hub/` config env today; keyring-backed storage is the
      upgrade path.

### Ops / QA

- [ ] **Push E2E on a real device** — subscribe/fan-out paths are unit-
      covered; a real browser push is unverified.
- [ ] ~~**Browser e2e**~~ — done: `crates/sepia-hub/tests/browser_e2e.rs`
      drives headless Chrome via thirtyfour + chromedriver over the real
      `sepia-node` → `sepia-hub` SSR stack (list → detail → actions).
      Ignored by default; run with
      `SEPIA_BROWSER_E2E=1 cargo test -p sepia-hub --test browser_e2e -- --ignored`.
      Needs chromedriver (`SEPIA_CHROMEDRIVER`, `target/webdriver/chromedriver`,
      or PATH — fetch the matching build from the chrome-for-testing
      `known-good-versions-with-downloads.json` endpoint) and Chrome
      (`CHROME_BIN` or PATH). A `browser-e2e` job in `ci.yml` runs it
      (continue-on-error — headless Chrome timing in CI is still being
      watched).
- [ ] **Unported surfaces** — managed-server registry, SSH tunnels,
      gateway proxy (`/api/servers`, `/api/gateway`), project transfer,
      OTEL. Intentionally out of scope for v0 — nodes must be directly
      reachable (`SEPIA_NODES` URLs); revisit when tailscale-less remote
      nodes matter.
- [ ] ~~wasm bundle in install~~ — done: `cargo xtask site` (also run
      by `install`) builds the wasm + bindgen output into `target/site`
      and stages it at `~/.local/share/sepia/site`; `sepia-hub` picks it
      up as its default site root. Requires `wasm-bindgen-cli` matching
      the locked crate version.
- [ ] **macOS verification** — all e2e ran on Linux; devin/cline path
      defaults and service install need a mac pass.
- [ ] **Body-cap audit** — large JSON handlers are capped; sweep for any
      remaining unbounded body read in `sepia-http`.

### UI/UX parity with the TS app

Ecosystem libs adopted (leptos-use `use_event_source`/`use_interval_fn`,
leptos_toaster; floating-ui-leptos + leptos_darkmode + leptos-fetch
available for the items below):

Landing (agents in flight):

- [x] Sidebar master-detail layout — list column + chat pane,
      `?session=` deep links, `/sessions/:id` standalone panel, mobile
      drawer nav (`Sheet`), list↔chat responsive swap
- [x] Right-side details drawer (rename, checkpoints, restore, delete)
      — `Sheet` opened from the panel's "Details" button
- [ ] Filter bar — search, agent multi-select, date (day/week/month),
      status (free/locked), sort (newest/oldest/title)
- [ ] Project grouping — collapsible sections in the sidebar list,
      persisted open state
- [ ] Hotkeys — N new, ↑/↓ session nav, ←/→ group fold, ⌘K filter,
      Esc clear, ⌘B sidebar, ⌘, settings, ? help

Missing (queued next):

- [ ] Row context menus — right-click → rename/pin/delete/details/
      add-to-project
- [ ] Icon set — TS used hugeicons; currently text glyphs only
- [ ] Chat polish — tool-call blocks w/ collapsible args, reasoning
      blocks, message scroller (auto-scroll pin + jump-to-bottom),
      syntax-highlighted code fences
- [ ] Prompt input — cwd autocomplete (CwdPicker), model picker,
      multiline textarea submit UX
- [ ] Theme toggle — light/dark/system (currently dark-only)
- [ ] Customizable keybinds + help dialog (settings override map)
- [ ] Settings depth — credentials, agent catalog, model config
      sections (was SettingsDialog)
- [ ] Session context tabs
- [ ] EmptyScreen/ErrorBanner parity

Intentionally dropped (v0):

- [ ] Managed servers, SSH tunnels, gateway proxy (`/api/servers`,
      `/api/gateway`) — nodes must be directly reachable
- [ ] Project transfer (push/pull) between nodes
- [ ] OTEL/telemetry hooks
- [ ] Per-browser node credentials (tokens live server-side now —
      this is a security improvement, not a regression)

## Done (Rust)

Whole-tree rewrite of the TS system: core IR + ports, four store drivers
as discovered binaries, tolerant ACP client, control plane (attach/
takeover/locks/idle sweep), axum REST+SSE, headless node, full CLI,
durable outbox + sync projection (subscribe-before-list, per-session
FIFO), Leptos SSR+hydrate UI with session ops, PWA shell + hub-owned
push fan-out, fault-injection + soak suites, real-subprocess e2e
(node↔driver↔agent↔hub). Credential model fixed by architecture: node
bearer tokens live in the hub config, never in the browser; the hub's
own browser surface is gated by `SEPIA_HUB_TOKEN` (bearer / `?token=` /
httpOnly cookie; required on non-loopback binds).
