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
- [x] Chat polish — ✓ collapsible tool-call blocks (args/result),
      ✓ reasoning blocks, ✓ scroller pin + "Jump to bottom", ✓ code-fence
      lang badge + copy button, ✓ keyword-level syntax highlighting
      (hand-rolled `sepia-web-core::highlight` tokenizer — comments/
      strings/numbers/keywords for the common fence langs; syntect
      deliberately skipped, mermaid still deferred)
- [~] Prompt input — ✓ auto-grow textarea, Shift+Enter hint, cwd chip,
      editable model input (`patch_meta {model}`, empty = agent
      default), attach button + image/file paste → ACP blocks,
      queued-write note. Deferred: real cwd autocomplete needs a node
      fs-list port; a model *select* needs models on the agent DTO
      (the wire advertises none — free-text field today).
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

- [x] Reconnecting indicator — the stream's `ready_state` feeds
      `sepia-web-core::stream::ReconnectTracker`: a "Reconnecting…"
      pill floats over the transcript only after a *post-open* drop
      (initial connect + detached stay quiet); always-mounted,
      class-toggled (SSR test asserts the container)
- [x] No-nodes-connected gating — the list renders "No nodes
      connected" (+ link to /nodes) when `node_status` shows nothing
      reachable; "No sessions yet" only when a node is up
- [x] Login form — `/login` page (card + token field) POSTs to the
      hub, which plants the httpOnly cookie and 303s to `next`; bad
      tokens bounce to `?error=1`. HTML navigations get a `/login`
      redirect (`?next=` preserved); `/api/*` + `/hub/*` keep JSON 401s;
      `GET /login?token=` stays the one-time-link bootstrap
- [x] Held-session send flow — the composer stays editable while
      held; send detours into a `ConfirmDialog` ("Take over and
      send") that `attach(takeover=true)`s then sends the draft —
      a refused/failed attach toasts and keeps the draft
- [x] Row `⋯` hover button — hover/focus reveals a `⋯` on every row
      (`[@media(hover:none)]` keeps it up on touch); opens the same
      menu as right-click, anchored at the pointer or the button's
      corner for keyboard opens; `aria-label` + `aria-haspopup=menu`

Missing features (wire-supported but no UI):

- [x] Session convert/export/import — details-sheet "Convert /
      Export" section (`POST /convert` → `{agent: cline|devin}`, then
      navigates to the copy; export is a plain `<a download>` through
      the hub `/api/*` passthrough); "Import session…" card under the
      new-session form (file → `POST /api/sessions/import`). Resume:
      no node endpoint exists — skipped.
- [x] Archive/unarchive — row-menu item via `patch_meta`; archived
      rows hide behind a "Show archived (N)" list-bottom toggle
- [x] Prompt attachments — paste/drop images or pick files (≤16
      blocks, ≤8MiB base64, node's caps); `image/*` → ACP `image`
      parts, other files → `resource` blob parts; chips above the
      textarea with ×-remove; attachments send with or without text
- [x] Reply/quote + per-message copy/rewind actions — ✓ hover strip
      on user/assistant rows: `copy` (navigator.clipboard) + `reply`
      (inserts the `> ` quote block into the draft via
      `composer::draft_with_quote` and focuses the textarea) +
      `rewind` — turns out the wire's `POST /rewind` already took a
      per-message `{nodeId}` selector (`RewindTarget::NodeId` keeps the
      node + prefix; `HistoryMessage.nodeId` is the aim), the old
      "checkpoint-scoped only" note was wrong. ConfirmDialog →
      `RewindSelector::NodeId`; gated on the new `canRewind` summary
      flag (the plane's registered `SessionRewinder`s — cursor has
      none) + not held/busy. Older nodes omit `canRewind` → hidden.
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
- [x] Per-session model override — the composer footer chip is now an
      editable input: commit → `PATCH {model: "…"}`, clear →
      `{model: null}` (agent default). No model *list* — the wire
      advertises none on `AgentDto`
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
- [x] Message auto-load on scroll-top + collapse consecutive dupes —
      ✓ scroll-top auto-load with scroll anchoring, ✓ `×N` fold on
      identical back-to-back rows, ✓ virtualized transcript — the
      `cv-auto` utility (`content-visibility: auto` +
      `contain-intrinsic-block-size: auto 10rem` + `flex-shrink: 0`,
      input.css) marks every transcript row; off-screen rows keep DOM
      but skip style/layout/paint. Chosen over JS windowing: identical
      SSR/hydrate markup, find-in-page/a11y/`<details>` state intact,
      pinned tail + prepend scroll-restore verified in headless Chrome
      (2k-row replica: FCP ~3.5×, load ~4.4× faster; `scrollTop =
      scrollHeight` still lands exactly). DOM windowing stays
      unwarranted — cv covers the layout/paint cost; DOM size is the
      remaining bound and page-windowed history already caps it
- [~] Markdown parity — ✓ GFM tables (`MdBlock::Table`, alignment,
      inline cells), ✓ keyword-level syntax highlighting
      (`sepia-web-core::highlight` — rust/python/js-ts/shell/json/
      toml/yaml/c-family/go/jvm/sql/css fences → Catppuccin token
      colors; unknown langs render plain). Deferred: mermaid — needs
      a JS lib, no Rust renderer exists
- [x] Toasts on mutation success — `ToastStore::outcome` adopted
      across the session surfaces: attach/detach/cancel/rename/delete/
      convert (detail), pin/archive/rename/delete (list rows), restore/
      rewind (checkpoint rows), model patch, import, attachment-stage
      failures; prompt-send failures toast too
- [~] a11y — ✓ Sheet/ConfirmDialog/`?` ShortcutsHelp: `role=dialog` +
      `aria-modal` + `aria-labelledby`/`aria-label`, initial focus +
      return-focus + Esc (`track_overlay_focus`; no full Tab trap —
      leptos-use 0.19 lacks `use_focus_trap`), ✓ toast live region
      (`role=status`, `aria-live=polite` inside the mounted
      `Toaster`), ✓ `prefers-reduced-motion` in input.css, ✓
      `role=combobox`/`listbox`/`option` + `aria-activedescendant` on
      the new-session cwd autocomplete, ✓ `aria-haspopup=menu` on the
      row `⋯` trigger. Left: keybind remapping, a real focus trap

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
