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
- [ ] ~~**Browser e2e**~~ — done: `crates/sepia-hub/tests/e2e/` drives
      headless Chrome via thirtyfour + chromedriver over a seeded
      `sepia-node` → `sepia-hub` SSR stack (SSR integrity, hydration
      console sweep, sessions, shell/hotkeys/theme). Run `cargo xtask
      e2e` (prebuilds binaries, sets the env gate). Needs chromedriver
      (`SEPIA_CHROMEDRIVER`, `target/webdriver/chromedriver`, or PATH)
      and Chrome (`CHROME_BIN` or PATH).
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

Ecosystem libs adopted — leptos-use (`use_event_source`,
`use_interval_fn`, `use_event_listener`, `on_click_outside`,
`use_local_storage`, `use_media_query`, `use_timeout_fn`),
leptos_toaster, floating-ui-leptos (menus/popovers), codee (SSE
codecs). Deliberately skipped: leptos_sse (syncs leptos *signals*;
ours is a domain SessionEvent stream), leptos_darkmode (bool-only,
no tri-state) and leptos_hotkeys (pins leptos 0.6). leptos-fetch:
adopt next pass — shared QueryClient replaces the Resource+manual
refetch plumbing (cross-component invalidation on SSE/ops).

Landing (agents in flight):

- [x] Sidebar master-detail layout — list column + chat pane,
      `?session=` deep links, `/sessions/:id` standalone panel, mobile
      drawer nav (`Sheet`), list↔chat responsive swap
- [x] Right-side details drawer (rename, checkpoints, restore, delete)
      — `Sheet` opened from the panel's "Details" button
- [x] Filter bar — search, agent multi-select, date (day/week/month),
      status (free/locked), sort (newest/oldest/title)
- [x] Project grouping — collapsible sections in the sidebar list,
      persisted open state
- [x] Hotkeys — N new, ↑/↓ session nav, ←/→ group fold, ⌘K filter,
      Esc clear, ⌘B sidebar, ⌘, settings, ? help

Missing (queued next):

- [x] Row context menus — right-click → open/rename/pin/delete/details
- [x] Icon set — inline lucide-style SVG Icon component (nav, pinned, actions)
- [~] Chat polish — ✓ collapsible tool-call blocks (args/result),
      ✓ reasoning blocks, ✓ scroller pin + "Jump to bottom", ✓ code-fence
      lang badge + copy button. Left: real syntax highlighting (needs a
      highlight crate)
- [~] Prompt input — ✓ auto-grow textarea, Shift+Enter hint, cwd/model
      chips, queued-write note. Deferred: real cwd autocomplete needs a
      node fs-list port; model picker needs models on the agent DTO.
- [x] Theme toggle — light/dark/system (`sepia-theme` localStorage +
      `use_media_query`, `.light` Catppuccin Latte token block, toggle
      in sidebar + mobile topbar; prose is `dark:prose-invert`)
- [~] Customizable keybinds + help dialog — ✓ `?` cheat-sheet landed;
      keybind *remapping* still open
- [x] Settings depth — appearance (theme select), node identity +
      connection, agent catalog, outbox writes, push, shortcuts table
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
