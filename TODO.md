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
- [x] Filter bar — ✓ search, ✓ status, ✓ sort, ✓ agent multi-select
      (checkbox dropdown), ✓ 24h/7d/30d recency chips, ✓ filter state
      URL-persisted (`?q=/agents=/status=/sort=/recency=/archived=`,
      deep-linkable on SSR)
- [x] Project grouping — collapsible sections in the sidebar list,
      persisted open state, right-click group menu (collapse + "New
      session here"), ←/→ fold hotkeys
- [x] Hotkeys — ✓ N new, ↑/↓ session nav, ⌘K palette, Esc clear, ⌘B
      sidebar, ? help, ✓ ←/→ group fold, ✓ ⌘, settings

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
- [x] Session context tabs — system rows fold into a ContextCard
      (Reports/Prompt/Rules/Skills) parsed in
      `sepia-web-core::history::parse_system_context`
- [ ] EmptyScreen/ErrorBanner parity — router fallback is a bare `<p>`;
      load failures render raw error text, no retry, no friendly 404/500

### Parity audit 2026-10 (from old TS bundle + reflog)

User-visible breakage:

- [ ] Reconnecting indicator — SSE auto-reconnects silently; user can't
      tell a dead stream from an idle session
- [x] No-nodes-connected gating — the list renders "No nodes
      connected" (+ link to /nodes) when `node_status` shows nothing
      reachable; "No sessions yet" only when a node is up
- [x] Login form — `/login` page (card + token field) POSTs to the
      hub, which plants the httpOnly cookie and 303s to `next`; bad
      tokens bounce to `?error=1`. HTML navigations get a `/login`
      redirect (`?next=` preserved); `/api/*` + `/hub/*` keep JSON 401s;
      `GET /login?token=` stays the one-time-link bootstrap
- [ ] Held-session send flow — composer isn't disabled, send on a held
      session just errors raw; old had draft-while-held + takeover
      confirm
- [ ] Row `⋯` hover button — actions are right-click only;
      undiscoverable on desktop, unusable on iOS/touch

Missing features (wire-supported but no UI):

- [ ] Session convert/export/import/resume (`/convert`, `/export`,
      `/import` exist on the node API; `sepia-convert` crate exists —
      unwired)
- [x] Archive/unarchive — row-menu item via `patch_meta`; archived
      rows hide behind a "Show archived (N)" list-bottom toggle
- [ ] Prompt attachments — node accepts `{text?, attachments?}`;
      composer sends text only
- [ ] Reply/quote + per-message copy/rewind actions
- [x] Usage footer — `HistoryMessageDto` decodes `usage`; assistant
      rows render ↑in/↓out/cost with cache+thinking on hover
- [x] Run/provenance marker rows — `spans` fold into the transcript
      as `agent @ node` dividers (live run start/end too)
- [x] Sub-agent badges — `↳` mark on child rows, `↳ N` child count
      on parents
- [~] Pinned/Recents sidebar sections — ✓ Pinned section (rows lifted
      out of groups); Recents deferred — duplicating rows across
      sections breaks the one-row-one-section invariant the arrow-key
      order + e2e row anchors rely on
- [ ] Per-session model picker (`patch_meta model` unwired; read-only
      chip today)
- [x] Per-tool renderers + live diffs — exec/read/edit/search/fetch/
      todo summaries, DiffBlock for recorded diffs, location chips,
      `contents` segments (`sepia-web-core::history::tool_summary`)
- [x] Project membership mgmt + details dialog + node nickname editing
      — project `Details` sheet lists sessions with membership
      checkboxes (`patch_meta {projectIds}` via `set_session_projects`),
      delete rides a page-level `ConfirmDialog`; `PATCH /api/node` is
      wired as `rename_node` and the inline `NicknameEdit` lives on
      /nodes identity card + health rows and /settings' node section
- [x] Command palette — ⌘K or `/` opens a `role=dialog` overlay:
      session search, page nav, actions (new session, theme, pin/
      archive selected, help); ↑/↓/Enter/Esc, aria-activedescendant,
      pure ranking in `sepia-web-core::palette`
- [x] Notification prefs — per-kind toggles (`done`, `permission`)
      under the push toggle; the wire honors `{prefs}` on
      `POST /api/push/subscribe` (endpoint-keyed upsert doubles as the
      update path). Gap: no prefs read-back endpoint, so the UI keeps
      its copy in localStorage (`sepia-notify-prefs`)
- [x] Pair-flow UI — "Pair a device" card on /nodes redeems a code via
      `POST /api/pair` (`NodeApi::pair` → `redeem_pair_code` server fn)
      and shows the minted node credential once
- [x] URL-persisted filter/sort state, group-header context menus
- [~] Message auto-load on scroll-top + collapse consecutive dupes —
      ✓ scroll-top auto-load with scroll anchoring, ✓ `×N` fold on
      identical back-to-back rows; virtualized list still open
- [~] Markdown parity — ✓ GFM tables (`MdBlock::Table`, alignment,
      inline cells); syntax highlighting + mermaid still open
- [~] Toasts on mutation success — done on settings/projects/nodes
      (create/delete project, config-set, pair, node rename, notify
      prefs); `ToastStore::outcome` helper is in `components.rs` for
      the session ops (rename/pin/delete/restore/rewind) which live in
      session_list/session_detail and await that workstream
- [~] a11y — ✓ Sheet/ConfirmDialog: `role=dialog` + `aria-modal` +
      `aria-labelledby`/`aria-label`, initial focus + return-focus +
      Esc (`track_overlay_focus`; no full Tab trap — leptos-use 0.19
      lacks `use_focus_trap`), ✓ toast live region (`role=status`,
      `aria-live=polite` inside the mounted `Toaster`), ✓
      `prefers-reduced-motion` in input.css. Left: `role=listbox` on
      the cwd combobox (session_detail.rs — other workstream), `?`
      sheet dialog role (app.rs — other workstream)

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
