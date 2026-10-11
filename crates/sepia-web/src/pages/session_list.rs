//! `/` — master-detail home. The session list lives in a left column
//! (sidebar-style on `lg+`, full-width below); selecting a row sets
//! `?session=<id>&agent=<agent>` and the chat pane mounts
//! `SessionPanel` inline. `/sessions/:id` redirects here.
//!
//! SSR renders the first page; on the client the list refreshes when
//! `/api/events` says a session row changed, and on a 30s ticker
//! (relative timestamps).

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
use sepia_web_core::composer::{CONVERT_TARGETS, import_body};
#[cfg(feature = "hydrate")]
use sepia_web_core::filter::query_href;
use sepia_web_core::filter::{
    CollapsedGroups, PINNED_KEY, Recency, SectionKind, SessionFilter, SessionRow, SortMode,
    StatusFilter, agent_options, child_counts, group_open, session_href_filtered,
};
#[cfg(feature = "hydrate")]
use sepia_web_core::filter::{fold_keys, session_href, visible_keys};
#[cfg(feature = "hydrate")]
use sepia_web_core::keymap::{self, KeyCtx, Mods, NavDir};
use sepia_web_core::path::{complete_path, non_empty, path_parts};

#[cfg(feature = "hydrate")]
use crate::api::list_agents;
use crate::api::{
    archive_session, create_session, delete_session, fs_dirs, import_session, pin_session,
    rename_session,
};
use crate::app::Now;
#[cfg(feature = "hydrate")]
use crate::app::in_editable;
use crate::components::icons::Icon;
use crate::components::toast::{ToastStore, use_toast};
use crate::components::{
    Badge, BadgeVariant, Button, ButtonClass, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState,
    ErrorBanner, Input, SELECT_CLASS, Sheet, SheetBody, SheetHeader, SheetTitle, Skeleton,
};
use crate::dto::AgentDto;
use crate::pages::SessionPanel;
use crate::time::relative;
use serde_json::Value;
use tw_merge::IntoTailwindClass;

/// Context-menu item styling — same look as `MenuItem` (the old app's
/// `rounded-2xl` menu-item recipe), minus the `<details>` close hook
/// (the row menu isn't a `<details>`).
const MENU_ITEM_CLS: &str = "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 text-sm font-medium hover:bg-accent cursor-pointer";
const MENU_ITEM_DESTRUCTIVE_CLS: &str = "flex w-full items-center gap-2.5 rounded-2xl px-3 py-2 \
                                        text-sm font-medium text-destructive \
                                        hover:bg-destructive/10 cursor-pointer";

/// Context-menu state — `(session id, agent, clientX, clientY)`.
/// `None` while the menu is closed.
type MenuTarget = Option<(String, Option<String>, f64, f64)>;

/// Group-header context-menu state — `(section key, clientX, clientY)`.
type GroupMenuTarget = Option<(String, f64, f64)>;

/// `/` — the Sessions section of the sidebar-09 shell: the panel
/// column (`SessionListPanel`) beside the detail pane. Selecting a
/// row sets `?session=<id>&agent=<agent>` and the chat pane mounts
/// `SessionPanel` inline; `/sessions/:id` deep links compose the
/// same pair (see `SessionDetailPage`).
#[component]
pub fn SessionListPage() -> impl IntoView {
    let query = use_query_map();
    // Selected session — `?session=<id>` (+ optional `?agent=`).
    let selected = move || {
        let q = query.read();
        let id = q.get("session").unwrap_or_default();
        if id.is_empty() {
            None
        } else {
            Some((id, {
                let a = q.get("agent").unwrap_or_default();
                (!a.is_empty()).then_some(a)
            }))
        }
    };
    let selected_id: Signal<Option<String>> = Signal::derive(move || selected().map(|(id, _)| id));

    view! {
        <Title text="sessions — sepia"/>
        <div class="flex h-full min-h-0 w-full">
            <SessionListPanel selected_id=selected_id/>
            // Chat pane — fills the rest on lg+; takes over the whole
            // screen below lg once a session is selected.
            {
                // `.into_any()` at each layout joint caps rustc's
                // view-type depth (the nested Suspense/Form tree
                // overflowed the query-depth limit in sepia-hub).
                view! {
            <div class=move || {
                let base = "min-w-0 flex-1 flex-col";
                if selected().is_some() {
                    format!("flex {base}")
                } else {
                    format!("hidden {base} lg:flex")
                }
            }>
                {move || {
                    match selected() {
                        // Re-evaluating produces a fresh view tree —
                        // SessionPanel unmounts/remounts per session, so
                        // its resources + SSE stream always target the
                        // current id.
                        Some((id, agent)) => {
                            view! { <SessionPanel session_id=id agent=agent/> }.into_any()
                        }
                        None => {
                            view! {
                                <EmptyState
                                    class="hidden lg:flex"
                                    title="Select a session"
                                    description="Pick a session from the list, or start a new one."
                                    icon="message-square-dashed"
                                />
                            }
                                .into_any()
                        }
                    }
                }}
            </div>
                }
                .into_any()
            }
        </div>
    }
}

/// The sidebar-09 Sessions panel — the content column that opens next
/// to the icon rail: panel header, new-session/import controls, the
/// filter bar, and the pinned/grouped session list (row + group
/// context menus and the delete confirm ride inside). Mounted by `/`
/// (master-detail) and `/sessions/:id` (deep links keep the list
/// beside the transcript). `selected_id` is the highlighted row +
/// "a session is open" flag — `?session=` on `/`, the `:id` param on
/// `/sessions/:id`. Below `lg` it's the full-width list while nothing
/// is selected; on `lg+` the rail's Sessions button or ⌘B collapses
/// it via the `PanelOpen` context (class-driven, so SSR and hydrate
/// share the DOM).
///
/// The component self-erases to `AnyView` (every heavy child is an
/// `AnyView` joint already, so the root type is shallow) — callers
/// embed `AnyView` instead of a page-sized concrete type. Splitting
/// the list out of `SessionListPage` added a component level to the
/// generic-instantiation chain, which is what tipped the wasm build
/// over rustc's query-depth limit; this keeps the ceiling lower than
/// the old single-component tree.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub fn SessionListPanel(#[prop(into)] selected_id: Signal<Option<String>>) -> impl IntoView {
    let client = crate::api::query_client();
    let sessions = client.resource(crate::api::sessions_scope, || ());
    let pending = client.resource(crate::api::pending_scope, || ());
    // Node health — the empty list reads "No nodes connected" instead
    // of "No sessions yet" when nothing is reachable.
    let nodes = client.resource(crate::api::node_status_scope, || ());
    let query = use_query_map();

    // Filter/sort state — seeded from the `?…` params so deep links
    // and reloads land on the same list. Both targets read the same
    // URL at setup, so SSR and first hydration emit identical DOM;
    // post-mount, an Effect writes changes back with `replace`.
    let initial_filter = SessionFilter::from_params(|k| query.read_untracked().get(k));
    let filter_text = RwSignal::new(initial_filter.query.clone());
    let sort_mode = RwSignal::new(initial_filter.sort);
    let status_filter = RwSignal::new(initial_filter.status);
    let agent_filter = RwSignal::new(initial_filter.agents.clone());
    let recency_filter = RwSignal::new(initial_filter.recency);
    let show_archived = RwSignal::new(initial_filter.show_archived);
    // The whole filter bar as one value — shared by the rendered
    // list, the arrow-key order, the row hrefs, and the URL sync.
    let current_filter = move || SessionFilter {
        query: filter_text.get(),
        status: status_filter.get(),
        agents: agent_filter.get(),
        sort: sort_mode.get(),
        recency: recency_filter.get(),
        show_archived: show_archived.get(),
    };
    // The `Now` clock feeds the recency window (30s ticker on the
    // client, request-time snapshot during SSR).
    let now = use_context::<Now>();
    let now_ms = move || now.map_or_else(crate::time::now_ms, |n| n.0.get());
    // Prefill for the new-session form's cwd field — "New session
    // here" in the group menu writes it.
    let new_cwd = RwSignal::new(String::new());
    // `⌘B` / the rail's Sessions button collapse this panel — the
    // open flag lives in the shell (`PanelOpen`) so the rail can
    // reach it; desktop-only (`lg:hidden` when closed).
    let panel_open = expect_context::<crate::app::PanelOpen>();
    // Cwds of collapsed project groups. Starts empty (all sections
    // open) on both targets; a post-hydration Effect adopts the set
    // persisted in localStorage, so collapse is purely class-driven.
    let closed_groups = RwSignal::new(CollapsedGroups::default());
    // The last resolved list — feeds the agent select (whose options
    // render empty on SSR + first hydration either way) and keyboard
    // navigation, which live outside the Suspend view tree.
    let all_sessions = RwSignal::new(Vec::<SessionRow>::new());
    // `n` focuses the new-session cwd input; `⌘K`/`Escape` the filter.
    let cwd_input_ref = NodeRef::<leptos::html::Input>::new();
    let filter_input_ref = NodeRef::<leptos::html::Input>::new();
    // Row context menu — one instance repositioned to the pointer:
    // `(session id, agent, clientX, clientY)`, `None` when closed.
    let menu_for: RwSignal<MenuTarget> = RwSignal::new(None);
    let menu_ref = NodeRef::<leptos::html::Div>::new();
    // `menu_active` is the `aria-activedescendant` index — the menu
    // container keeps DOM focus (palette pattern, not roving
    // tabindex); the signal names the highlighted item.
    let menu_active = RwSignal::new(0usize);
    // Group-header context menu — same pattern, keyed by section.
    let group_menu: RwSignal<GroupMenuTarget> = RwSignal::new(None);
    let group_menu_ref = NodeRef::<leptos::html::Div>::new();
    let group_menu_active = RwSignal::new(0usize);
    // Inline rename — the id whose row title is swapped for an input.
    let renaming: RwSignal<Option<String>> = RwSignal::new(None);
    let rename_draft = RwSignal::new(String::new());
    // Delete flow — the dialog reads `pending_delete`; see the flag +
    // Effect dance below (`ConfirmDialog::on_confirm` must be `Send`,
    // `use_navigate` isn't).
    let confirm_delete = RwSignal::new(false);
    let delete_confirmed = RwSignal::new(false);
    let pending_delete: RwSignal<Option<(String, Option<String>)>> = RwSignal::new(None);
    // `new_local` stores a SendWrapper in the owner arena — on SSR the
    // owner cleans up on an arbitrary tokio worker and the guard
    // panics, aborting the stream. SSR never navigates.
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});

    let toast = use_toast();

    // Runs inside the component owner, so `!Send` captures are fine.
    let do_delete = move || {
        let Some((id, agent)) = pending_delete.get_untracked() else {
            return;
        };
        pending_delete.set(None);
        leptos::task::spawn_local(async move {
            if toast
                .outcome(delete_session(id, agent).await, "Deleted")
                .is_some()
            {
                sessions.refetch();
                navigate.with_value(|n| n("/", NavigateOptions::default()));
            }
        });
    };
    Effect::new(move |_| {
        if delete_confirmed.get() {
            delete_confirmed.set(false);
            do_delete();
        }
    });

    // Ordered `(id, agent)` of the rows currently on screen —
    // filtered, sorted, sectioned, and with collapsed sections
    // skipped — so ArrowUp/ArrowDown can move the `?session=`
    // selection.
    #[cfg(feature = "hydrate")]
    let visible_order = move || -> Vec<(String, String)> {
        let filter = SessionFilter {
            query: filter_text.get_untracked(),
            status: status_filter.get_untracked(),
            agents: agent_filter.get_untracked(),
            sort: sort_mode.get_untracked(),
            recency: recency_filter.get_untracked(),
            show_archived: show_archived.get_untracked(),
        };
        let sections = filter.sections(all_sessions.get_untracked(), now_ms());
        visible_keys(
            &sections,
            &closed_groups.get_untracked(),
            filter.force_open(),
        )
    };

    // Refresh when the node feed reports a session/meta/project change.
    #[cfg(feature = "hydrate")]
    {
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            sessions.refetch();
            pending.refetch();
            nodes.refetch();
        }));
        crate::app::every_ms(30_000, move || {
            pending.refetch();
            nodes.refetch();
        });
        // Clicking anywhere outside the menus closes them (the
        // right-click that opens one precedes `contextmenu`, not a
        // `click`, so it can't immediately re-close).
        let _outside = leptos_use::on_click_outside(menu_ref, move |_| menu_for.set(None));
        let _outside2 = leptos_use::on_click_outside(group_menu_ref, move |_| group_menu.set(None));
        // Menu focus on open (first enabled item + container focus)
        // and back to the opener on close — the roving
        // `aria-activedescendant` half of WAI-ARIA menu practice.
        track_menu_focus(move || menu_for.read().is_some(), menu_ref, menu_active);
        track_menu_focus(
            move || group_menu.read().is_some(),
            group_menu_ref,
            group_menu_active,
        );
    }

    // Filter bar → URL. First run is adoption (the params already
    // seeded the signals); later runs `replace` so typing a query
    // doesn't flood history. `session`/`agent` are preserved — the
    // selection isn't the filter's to drop — and the href keeps the
    // current pathname: on `/sessions/:id` deep links the filter
    // syncs onto that URL instead of bouncing back to `/`.
    #[cfg(feature = "hydrate")]
    {
        let location = leptos_router::hooks::use_location();
        let mut init = true;
        Effect::new(move |_| {
            let f = current_filter();
            if std::mem::replace(&mut init, false) {
                return;
            }
            let q = query.read_untracked();
            let mut pairs = Vec::new();
            if let Some(s) = q.get("session") {
                pairs.push(("session".to_string(), s));
            }
            if let Some(a) = q.get("agent") {
                pairs.push(("agent".to_string(), a));
            }
            pairs.extend(f.query_params());
            let href = query_href(&location.pathname.get_untracked(), &pairs);
            navigate.with_value(|n| {
                n(
                    &href,
                    NavigateOptions {
                        replace: true,
                        scroll: false,
                        ..NavigateOptions::default()
                    },
                );
            });
        });
    }

    // Persist collapsed groups in localStorage. The Effect's first run
    // adopts the stored set (post-hydration only — SSR stays open);
    // subsequent runs write toggles back.
    #[cfg(feature = "hydrate")]
    {
        let (stored, set_stored, _clear) = leptos_use::storage::use_local_storage::<
            CollapsedGroups,
            codee::string::JsonSerdeCodec,
        >("sepia-list-collapsed");
        let mut init = true;
        Effect::new(move |_| {
            let cur = closed_groups.get();
            if std::mem::replace(&mut init, false) {
                closed_groups.set(stored.get_untracked());
            } else {
                set_stored.set(cur);
            }
        });
    }

    // Global hotkeys — document keydown, client only. The key→action
    // mapping is pure (`sepia_web_core::keymap`, resolved through the
    // `keybinds` context so per-browser overrides apply); this
    // listener owns the list-page actions — `ToggleHelp`/`CloseHelp`
    // belong to the shell's listener in `app.rs`.
    #[cfg(feature = "hydrate")]
    {
        let navigate = use_navigate();
        let keybinds = crate::keybinds::use_keybinds();
        let _stop = leptos_use::use_event_listener(
            leptos::prelude::document(),
            leptos::ev::keydown,
            move |ev: web_sys::KeyboardEvent| {
                let mods = Mods {
                    ctrl: ev.ctrl_key(),
                    meta: ev.meta_key(),
                    alt: ev.alt_key(),
                    shift: ev.shift_key(),
                };
                let filter_focused = filter_input_ref
                    .get()
                    .zip(leptos::prelude::document().active_element())
                    .is_some_and(|(el, active)| {
                        use wasm_bindgen::JsCast;
                        active == *el.unchecked_ref::<web_sys::Element>()
                    });
                let ctx = KeyCtx {
                    typing: in_editable(&ev),
                    filter_focused,
                    menu_open: menu_for.get_untracked().is_some()
                        || group_menu.get_untracked().is_some(),
                    ..KeyCtx::default()
                };
                let Some(action) = keybinds.resolve(&ev.key(), mods, ctx) else {
                    return;
                };
                if action.prevent_default() {
                    ev.prevent_default();
                }
                match action {
                    // `n` — jump to the new-session cwd input.
                    keymap::Action::FocusNewSession => {
                        if let Some(el) = cwd_input_ref.get() {
                            let _ = el.focus();
                        }
                    }
                    // Escape cascade — row menu first, then clear +
                    // blur the focused filter, otherwise drop the
                    // `?session=` selection. (The inline rename input
                    // stops propagation on its own Escape.)
                    keymap::Action::CloseMenu => {
                        menu_for.set(None);
                        group_menu.set(None);
                    }
                    keymap::Action::ClearFilter => {
                        filter_text.set(String::new());
                        if let Some(el) = filter_input_ref.get() {
                            let _ = el.blur();
                        }
                    }
                    keymap::Action::CloseSelection => {
                        navigate("/", NavigateOptions::default());
                    }
                    // Arrow keys — move `?session=` through the visible
                    // row order (wrapping at the ends).
                    keymap::Action::NavNext | keymap::Action::NavPrev => {
                        let order = visible_order();
                        let cur = selected_id.get_untracked();
                        let idx = cur
                            .as_deref()
                            .and_then(|id| order.iter().position(|(sid, _)| sid == id));
                        let dir = if action == keymap::Action::NavNext {
                            NavDir::Down
                        } else {
                            NavDir::Up
                        };
                        if let Some(next) = keymap::nav_index(idx, order.len(), dir) {
                            let (id, agent) = &order[next];
                            navigate(&session_href(id, agent), NavigateOptions::default());
                        }
                    }
                    // `←`/`→` — fold/unfold the selected row's
                    // section, or every section with no selection.
                    keymap::Action::FoldGroup | keymap::Action::UnfoldGroup => {
                        let filter = SessionFilter {
                            query: filter_text.get_untracked(),
                            status: status_filter.get_untracked(),
                            agents: agent_filter.get_untracked(),
                            sort: sort_mode.get_untracked(),
                            recency: recency_filter.get_untracked(),
                            show_archived: show_archived.get_untracked(),
                        };
                        let sections = filter.sections(all_sessions.get_untracked(), now_ms());
                        let selected = selected_id.get_untracked();
                        let keys = fold_keys(&sections, selected.as_deref());
                        let fold = action == keymap::Action::FoldGroup;
                        closed_groups.update(|cg| {
                            // Unfold-all clears outright — it also
                            // prunes keys whose cwd left the list.
                            if !fold && selected.is_none() {
                                cg.clear();
                            }
                            for key in keys {
                                cg.set_closed(&key, fold);
                            }
                        });
                    }
                    // Owned by the shell listener (`app.rs`) — ⌘K/`/`
                    // open the command palette now, not the filter;
                    // ⌘B toggles this panel through `PanelOpen`.
                    keymap::Action::ToggleHelp
                    | keymap::Action::CloseHelp
                    | keymap::Action::TogglePalette
                    | keymap::Action::ToggleList
                    | keymap::Action::OpenSettings => {}
                }
            },
        );
    }

    view! {
        // List column — the sidebar-09 Sessions panel beside the rail
        // on lg+, full page below that (where it hides once a session
        // is selected). Menus + the delete confirm are fixed/hidden
        // overlays; they live inside this root.
        <div
            id="sessions-panel"
            data-name="SessionsPanel"
            class=move || {
                let base = "min-w-0 flex-col overflow-y-auto bg-sidebar text-sidebar-foreground lg:flex lg:w-80 lg:shrink-0 lg:border-r lg:border-sidebar-border";
                // `⌘B` / rail collapse is desktop-only (`lg:hidden`) —
                // on small screens the column stays usable.
                let collapse = if panel_open.0.get() { "" } else { " lg:hidden" };
                if selected_id.get().is_some() {
                    format!("hidden {base}{collapse}")
                } else {
                    format!("flex {base} flex-1{collapse}")
                }
            }
        >
            // Panel header — the sidebar-09 section title + the same
            // collapse affordance the rail button and ⌘B drive.
            <div class="flex h-11 shrink-0 items-center justify-between border-b border-sidebar-border px-3">
                <span class="text-sm font-semibold">"Sessions"</span>
                <button
                    type="button"
                    aria-label="Collapse the sessions panel"
                    title="Collapse the sessions panel (⌘B)"
                    class="hidden size-7 items-center justify-center rounded-md text-sidebar-foreground/60 transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground lg:inline-flex"
                    on:click=move |_| panel_open.0.update(|o| *o = !*o)
                >
                    <Icon name="panel-left-close" class="size-4"/>
                </button>
            </div>
            {
                // `.into_any()` at each layout joint caps rustc's
                // view-type depth — the form/import subtree is the
                // deepest chain in this file (nested `into_any`
                // instantiations across a component boundary overflow
                // the query-depth limit in the wasm build).
                view! {
            <div class="border-b p-3">
                <NewSessionForm
                    cwd_ref=cwd_input_ref
                    cwd=new_cwd
                    on_created=move || sessions.refetch()
                />
                <ImportCard on_imported=move || sessions.refetch()/>
            </div>
                }
                .into_any()
            }
            {
                // Filter bar — client-side only. The agent options read
                // `all_sessions`, which is empty until the Suspend below
                // resolves, so SSR and first hydration emit the same
                // empty checkbox list (like the agent select in
                // NewSessionForm). `.into_any()` caps this joint's
                // view-type depth like the layout joints above.
                view! {
            <div class="border-b p-2">
                    <div class="flex flex-col gap-1.5">
                        // `{..}` marks everything after it as spread
                        // attrs on the component's root element —
                        // `node_ref` only works that way on components.
                        <Input
                            {..}
                            node_ref=filter_input_ref
                            attr:r#type="text"
                            attr:placeholder="Filter sessions…"
                            prop:value=move || filter_text.get()
                            on:input=move |ev| filter_text.set(event_target_value(&ev))
                        />
                        <div class="flex gap-1.5">
                            <select
                                class=format!("{SELECT_CLASS} flex-1")
                                prop:value=move || sort_mode.get().as_str()
                                on:change=move |ev| {
                                    sort_mode.set(SortMode::parse(&event_target_value(&ev)));
                                }
                            >
                                <option value="newest">"newest"</option>
                                <option value="oldest">"oldest"</option>
                                <option value="title">"title"</option>
                            </select>
                            <select
                                class=format!("{SELECT_CLASS} flex-1")
                                prop:value=move || status_filter.get().as_str()
                                on:change=move |ev| {
                                    status_filter.set(StatusFilter::parse(&event_target_value(&ev)));
                                }
                            >
                                <option value="all">"any"</option>
                                <option value="free">"free"</option>
                                <option value="locked">"locked"</option>
                            </select>
                            <AgentFilter rows=all_sessions selected=agent_filter/>
                        </div>
                        // Recency chips — fixed 24h/7d/30d windows on
                        // `updated_at`.
                        <div class="flex gap-1" data-name="RecencyFilter">
                            {[
                                (Recency::Any, "any"),
                                (Recency::Day, "24h"),
                                (Recency::Week, "7d"),
                                (Recency::Month, "30d"),
                            ]
                                .into_iter()
                                .map(|(r, label)| {
                                    view! {
                                        <button
                                            type="button"
                                            data-name="RecencyChip"
                                            data-active=move || {
                                                (recency_filter.get() == r).then_some("")
                                            }
                                            class=move || {
                                                let base = "rounded-md border px-2 py-0.5 text-[11px]";
                                                if recency_filter.get() == r {
                                                    format!("{base} border-border bg-accent text-foreground")
                                                } else {
                                                    format!("{base} border-transparent text-muted-foreground hover:text-foreground")
                                                }
                                            }
                                            on:click=move |_| recency_filter.set(r)
                                        >
                                            {label}
                                        </button>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>
                </div>
                    }
                    .into_any()
                }
            // `min-h-0` lets the scroll area shrink inside the column
            // instead of pushing the header/form off-screen.
            <div class="flex min-h-0 flex-1 overflow-y-auto p-2">
                {
                    // View-type erasure — the nested Suspense tree
                    // overflows rustc's query depth when the page is
                    // one giant concrete type. `.into_any()` caps it.
                    view! {
                    <Suspense fallback=move || {
                        view! {
                            <div class="flex flex-col gap-1.5 p-1">
                                <Skeleton class="h-14 w-full"/>
                                <Skeleton class="h-14 w-full"/>
                                <Skeleton class="h-14 w-full"/>
                            </div>
                        }
                    }>
                        {move || {
                            Suspend::new(async move {
                                // A broken outbox/status read never
                                // sinks the list.
                                let pending_result = pending.await.unwrap_or_default();
                                let status_rows = nodes.await.unwrap_or_default();
                                match sessions.await {
                                    Err(e) => {
                                        all_sessions.set(Vec::new());
                                        view! {
                                            <ErrorBanner
                                                message=e.to_string()
                                                on_retry=Box::new(move || sessions.refetch())
                                            />
                                        }
                                            .into_any()
                                    }
                                    Ok(list) if list.is_empty() => {
                                        all_sessions.set(Vec::new());
                                        // No reachable node makes
                                        // "no sessions" misleading —
                                        // the node itself is the
                                        // missing piece.
                                        if status_rows.iter().any(|r| r.status == "up") {
                                            view! {
                                                <p class="p-2 text-sm text-muted-foreground">
                                                    "No sessions yet."
                                                </p>
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <EmptyState
                                                    icon="cloud-off"
                                                    title="No nodes connected"
                                                    description="Sessions appear here once a node is reachable. Check node health or pair a device."
                                                >
                                                    <A
                                                        href="/nodes"
                                                        attr:class=ButtonClass {
                                                            variant: ButtonVariant::Secondary,
                                                            size: ButtonSize::Sm,
                                                        }
                                                        .to_class()
                                                    >
                                                        "Open nodes"
                                                    </A>
                                                </EmptyState>
                                            }
                                                .into_any()
                                        }
                                    }
                                    Ok(list) => {
                                        // The whole view pipeline runs on
                                        // the lightweight `SessionRow`.
                                        let list: Vec<SessionRow> =
                                            list.iter().map(SessionRow::from).collect();
                                        // `(queued, failed)` per session id.
                                        let mut writes: BTreeMap<String, (usize, usize)> =
                                            BTreeMap::new();
                                        for w in &pending_result {
                                            let entry =
                                                writes.entry(w.session_id.clone()).or_default();
                                            if w.status == "failed" {
                                                entry.1 += 1;
                                            } else {
                                                entry.0 += 1;
                                            }
                                        }
                                        // Publish for the agent select +
                                        // arrow-key order (same value on
                                        // SSR and hydrate).
                                        all_sessions.set(list.clone());
                                        view! {
                                            {move || {
                                                let filter = current_filter();
                                                let sections =
                                                    filter.sections(list.clone(), now_ms());
                                                let shown: usize =
                                                    sections.iter().map(|s| s.rows.len()).sum();
                                                // Hidden archived rows count
                                                // against the heading's "of M"
                                                // only while shown.
                                                let total = list
                                                    .iter()
                                                    .filter(|s| {
                                                        filter.show_archived || !s.archived
                                                    })
                                                    .count();
                                                let archived_total =
                                                    list.iter().filter(|s| s.archived).count();
                                                // A text filter force-expands
                                                // every section.
                                                let force_open = filter.force_open();
                                                let heading = filter.heading(shown, total);
                                                let kids = child_counts(&list);
                                                let row_filter = filter.clone();
                                                let sections = sections
                                                    .into_iter()
                                                    .map(|section| {
                                                        let count = section.rows.len();
                                                        let label = section.label.clone();
                                                        let pinned_hdr =
                                                            section.kind == SectionKind::Pinned;
                                                        // Class-driven collapse —
                                                        // the signal flips the
                                                        // `<ul>` class without
                                                        // re-rendering the rows.
                                                        let closed = Signal::derive({
                                                            let key = section.key.clone();
                                                            move || {
                                                                !group_open(
                                                                    &key,
                                                                    &closed_groups.read(),
                                                                    force_open,
                                                                )
                                                            }
                                                        });
                                                        let toggle_key = section.key.clone();
                                                        let menu_key = section.key.clone();
                                                        let expanded_key = section.key.clone();
                                                        let rows = section
                                                            .rows
                                                            .into_iter()
                                                            .map(|s| {
                                                                let (queued, failed) = writes
                                                                    .get(&s.id)
                                                                    .copied()
                                                                    .unwrap_or_default();
                                                                let children = kids
                                                                    .get(&s.id)
                                                                    .copied()
                                                                    .unwrap_or_default();
                                                                let row_id = s.id.clone();
                                                                let is_selected =
                                                                    Signal::derive(move || {
                                                                        selected_id
                                                                            .get()
                                                                            .as_deref()
                                                                            == Some(row_id.as_str())
                                                                    });
                                                                let href =
                                                                    session_href_filtered(
                                                                        &row_filter,
                                                                        &s.id,
                                                                        &s.agent,
                                                                    );
                                                                view! {
                                                                    <SessionRowView
                                                                        session=s
                                                                        href=href
                                                                        children=children
                                                                        queued=queued
                                                                        failed=failed
                                                                        selected=is_selected
                                                                        menu=menu_for
                                                                        renaming=renaming
                                                                        rename_draft=rename_draft
                                                                        on_changed=move || {
                                                                            sessions.refetch();
                                                                        }
                                                                    />
                                                                }
                                                            })
                                                            .collect::<Vec<_>>();
                                                        view! {
                                                            <section>
                                                                <button
                                                                    type="button"
                                                                    data-name="GroupHeader"
                                                                    aria-haspopup="menu"
                                                                    aria-controls="sepia-group-menu"
                                                                    aria-expanded=move || {
                                                                        group_menu
                                                                            .read()
                                                                            .as_ref()
                                                                            .is_some_and(|(k, _, _)| *k == expanded_key)
                                                                            .to_string()
                                                                    }
                                                                    class="flex w-full items-center gap-1.5 rounded-md px-1.5 py-0.5 text-left text-sm font-medium text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                                                                    on:click=move |_| {
                                                                        closed_groups.update(|v| {
                                                                            v.toggle(&toggle_key);
                                                                        });
                                                                    }
                                                                    on:contextmenu=move |ev| {
                                                                        ev.prevent_default();
                                                                        group_menu.set(Some((
                                                                            menu_key.clone(),
                                                                            f64::from(ev.client_x()),
                                                                            f64::from(ev.client_y()),
                                                                        )));
                                                                    }
                                                                >
                                                                    <span class="inline-flex w-3 shrink-0 items-center">
                                                                        {move || {
                                                                            if closed.get() {
                                                                                view! {
                                                                                    <Icon
                                                                                        name="chevron-right"
                                                                                        class="size-3"
                                                                                    />
                                                                                }
                                                                                    .into_any()
                                                                            } else {
                                                                                view! {
                                                                                    <Icon
                                                                                        name="chevron-down"
                                                                                        class="size-3"
                                                                                    />
                                                                                }
                                                                                    .into_any()
                                                                            }
                                                                        }}
                                                                    </span>
                                                                    {pinned_hdr.then(|| {
                                                                        view! {
                                                                            <Icon
                                                                                name="pin"
                                                                                class="size-3 shrink-0"
                                                                            />
                                                                        }
                                                                    })}
                                                                    <span class="min-w-0 truncate">
                                                                        {label}
                                                                    </span>
                                                                    <span class="ml-auto shrink-0 font-normal normal-case">
                                                                        {count}
                                                                    </span>
                                                                </button>
                                                                // Class-driven collapse — the rows
                                                                // always render, so SSR and hydrate
                                                                // share the same DOM; only the class
                                                                // flips post-mount.
                                                                <ul class=move || {
                                                                    if closed.get() {
                                                                        "hidden"
                                                                    } else {
                                                                        "flex flex-col gap-0.5"
                                                                    }
                                                                }>
                                                                    {rows}
                                                                </ul>
                                                            </section>
                                                        }
                                                            .into_any()
                                                    })
                                                    .collect::<Vec<_>>();
                                                let body: AnyView = if sections.is_empty() {
                                                    view! {
                                                        <p class="p-2 text-sm text-muted-foreground">
                                                            "No sessions match."
                                                        </p>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <div class="flex flex-col gap-1.5">
                                                            {sections}
                                                        </div>
                                                    }
                                                        .into_any()
                                                };
                                                view! {
                                                    <p class="px-1.5 pb-1 text-[11px] text-muted-foreground">
                                                        {heading}
                                                    </p>
                                                    {body}
                                                    // Archived rows are hidden
                                                    // by default; the toggle
                                                    // widens the list, it
                                                    // doesn't filter it.
                                                    {(archived_total > 0).then(|| {
                                                        view! {
                                                            <button
                                                                type="button"
                                                                data-name="ArchivedToggle"
                                                                class="mt-1 w-full rounded-sm px-1.5 py-1 text-left text-[11px] text-muted-foreground hover:text-foreground"
                                                                on:click=move |_| {
                                                                    show_archived.update(|v| *v = !*v);
                                                                }
                                                            >
                                                                {move || {
                                                                    if show_archived.get() {
                                                                        "Hide archived".to_string()
                                                                    } else {
                                                                        format!(
                                                                            "Show archived ({archived_total})"
                                                                        )
                                                                    }
                                                                }}
                                                            </button>
                                                        }
                                                    })}
                                                }
                                                    .into_any()
                                            }}
                                        }
                                            .into_any()
                                    }
                                }
                            })
                        }}
                    </Suspense>
                        }
                        .into_any()
                    }
            </div>
            // Row context menu — a single fixed-position instance moved
            // to the pointer on `contextmenu`. Always rendered (`hidden`
            // while closed) so SSR and hydrate emit identical DOM;
            // closes on outside click and Escape.
            //
            // a11y: the container keeps DOM focus (`tabindex="-1"` +
            // `aria-activedescendant`, the palette's pattern — no
            // roving tabindex on items); arrows/Home/End/typeahead
            // move `menu_active`, Enter/Space clicks it, Escape
            // returns focus to the opener (`track_menu_focus` +
            // `menu_keydown`). Item clicks can't steal focus —
            // `mousedown` is eaten on the container.
            {
                view! {
            <div
                node_ref=menu_ref
                role="menu"
                id="sepia-row-menu"
                aria-label="Session actions"
                tabindex="-1"
                aria-activedescendant=move || {
                    menu_for
                        .read()
                        .is_some()
                        .then(|| format!("sepia-row-menu-item-{}", menu_active.get()))
                }
                class=move || {
                    if menu_for.read().is_some() {
                        "fixed z-50 w-44 rounded-3xl bg-popover p-1.5 text-popover-foreground shadow-lg ring-1 ring-border focus:outline-none"
                    } else {
                        "hidden"
                    }
                }
                style=move || {
                    menu_for.get().map_or_else(String::new, |(_, _, x, y)| {
                        // CSS `min()` keeps the menu inside the viewport.
                        format!(
                            "left:min({x}px, calc(100vw - 12rem)); top:min({y}px, calc(100vh - 14rem))"
                        )
                    })
                }
                on:mousedown=move |ev| ev.prevent_default()
                on:keydown=move |ev| {
                    menu_keydown(&ev, menu_ref, menu_active, move || menu_for.set(None));
                }
            >
                {move || {
                    menu_for.get().map(|(id, agent, _x, _y)| {
                        let open_href = session_href_filtered(
                            &current_filter(),
                            &id,
                            agent.as_deref().unwrap_or_default(),
                        );
                        let detail_href = match &agent {
                            Some(a) => format!("/sessions/{id}?agent={a}"),
                            None => format!("/sessions/{id}"),
                        };
                        let (pinned_now, archived_now) = all_sessions
                            .read()
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| (s.pinned, s.archived))
                            .unwrap_or_default();
                        view! {
                            <CtxMenuItem
                                id="sepia-row-menu-item-0"
                                index=0
                                active=menu_active
                                on_click=Box::new({
                                    let href = open_href.clone();
                                    move || {
                                        menu_for.set(None);
                                        navigate.with_value(|n| {
                                            n(&href, NavigateOptions::default());
                                        });
                                    }
                                })
                            >
                                "Open"
                            </CtxMenuItem>
                            <CtxMenuItem
                                id="sepia-row-menu-item-1"
                                index=1
                                active=menu_active
                                on_click=Box::new({
                                    let id = id.clone();
                                    move || {
                                        menu_for.set(None);
                                        let draft = all_sessions
                                            .read()
                                            .iter()
                                            .find(|s| s.id == id)
                                            .map_or_else(String::new, |s| s.title.clone());
                                        rename_draft.set(draft);
                                        renaming.set(Some(id.clone()));
                                    }
                                })
                            >
                                "Rename…"
                            </CtxMenuItem>
                            <CtxMenuItem
                                id="sepia-row-menu-item-2"
                                index=2
                                active=menu_active
                                on_click=Box::new({
                                    let id = id.clone();
                                    let agent = agent.clone();
                                    move || {
                                        menu_for.set(None);
                                        let id = id.clone();
                                        let agent = agent.clone();
                                        leptos::task::spawn_local(async move {
                                            let ok = if pinned_now {
                                                "Unpinned"
                                            } else {
                                                "Pinned"
                                            };
                                            if toast
                                                .outcome(
                                                    pin_session(id, agent, !pinned_now).await,
                                                    ok,
                                                )
                                                .is_some()
                                            {
                                                sessions.refetch();
                                            }
                                        });
                                    }
                                })
                            >
                                {if pinned_now { "Unpin" } else { "Pin" }}
                            </CtxMenuItem>
                            <CtxMenuItem
                                id="sepia-row-menu-item-3"
                                index=3
                                active=menu_active
                                on_click=Box::new({
                                    let id = id.clone();
                                    let agent = agent.clone();
                                    move || {
                                        menu_for.set(None);
                                        let id = id.clone();
                                        let agent = agent.clone();
                                        leptos::task::spawn_local(async move {
                                            let ok = if archived_now {
                                                "Unarchived"
                                            } else {
                                                "Archived"
                                            };
                                            if toast
                                                .outcome(
                                                    archive_session(id, agent, !archived_now).await,
                                                    ok,
                                                )
                                                .is_some()
                                            {
                                                sessions.refetch();
                                            }
                                        });
                                    }
                                })
                            >
                                {if archived_now { "Unarchive" } else { "Archive" }}
                            </CtxMenuItem>
                            <CtxMenuItem
                                id="sepia-row-menu-item-4"
                                index=4
                                active=menu_active
                                destructive=true
                                on_click=Box::new({
                                    let id = id.clone();
                                    let agent = agent.clone();
                                    move || {
                                        menu_for.set(None);
                                        pending_delete.set(Some((id.clone(), agent.clone())));
                                        confirm_delete.set(true);
                                    }
                                })
                            >
                                "Delete…"
                            </CtxMenuItem>
                            <CtxMenuItem
                                id="sepia-row-menu-item-5"
                                index=5
                                active=menu_active
                                on_click=Box::new({
                                    let href = detail_href.clone();
                                    move || {
                                        menu_for.set(None);
                                        navigate.with_value(|n| {
                                            n(&href, NavigateOptions::default());
                                        });
                                    }
                                })
                            >
                                "Details"
                            </CtxMenuItem>
                        }
                    })
                }}
            </div>
                }
                .into_any()
            }
            // Group-header context menu — collapse/expand the section,
            // or start a session in its cwd. Same fixed-position
            // always-rendered pattern + `aria-activedescendant`
            // keyboard nav as the row menu.
            {
                view! {
            <div
                node_ref=group_menu_ref
                role="menu"
                id="sepia-group-menu"
                aria-label="Group actions"
                tabindex="-1"
                aria-activedescendant=move || {
                    group_menu
                        .read()
                        .is_some()
                        .then(|| format!("sepia-group-menu-item-{}", group_menu_active.get()))
                }
                data-name="GroupMenu"
                data-open=move || group_menu.read().is_some().then_some("")
                class=move || {
                    if group_menu.read().is_some() {
                        "fixed z-50 w-44 rounded-3xl bg-popover p-1.5 text-popover-foreground shadow-lg ring-1 ring-border focus:outline-none"
                    } else {
                        "hidden"
                    }
                }
                style=move || {
                    group_menu.get().map_or_else(String::new, |(_, x, y)| {
                        format!(
                            "left:min({x}px, calc(100vw - 12rem)); top:min({y}px, calc(100vh - 10rem))"
                        )
                    })
                }
                on:mousedown=move |ev| ev.prevent_default()
                on:keydown=move |ev| {
                    menu_keydown(&ev, group_menu_ref, group_menu_active, move || {
                        group_menu.set(None);
                    });
                }
            >
                {move || {
                    group_menu.get().map(|(key, _x, _y)| {
                        let toggle_key = key.clone();
                        let closed_label = move || {
                            let open = group_open(
                                &toggle_key,
                                &closed_groups.read(),
                                current_filter().force_open(),
                            );
                            if open { "Collapse" } else { "Expand" }
                        };
                        // "New session here" needs a real cwd — the
                        // pinned pseudo-section and the empty-cwd
                        // group don't offer it.
                        let can_spawn = key != PINNED_KEY && !key.is_empty();
                        view! {
                            <CtxMenuItem
                                id="sepia-group-menu-item-0"
                                index=0
                                active=group_menu_active
                                on_click=Box::new({
                                    let key = key.clone();
                                    move || {
                                        group_menu.set(None);
                                        closed_groups.update(|v| v.toggle(&key));
                                    }
                                })
                            >
                                {closed_label}
                            </CtxMenuItem>
                            {can_spawn.then(|| {
                                let key = key.clone();
                                view! {
                                    <CtxMenuItem
                                        id="sepia-group-menu-item-1"
                                        index=1
                                        active=group_menu_active
                                        on_click=Box::new(move || {
                                            group_menu.set(None);
                                            new_cwd.set(key.clone());
                                            if let Some(el) = cwd_input_ref.get() {
                                                let _ = el.focus();
                                            }
                                        })
                                    >
                                        "New session here"
                                    </CtxMenuItem>
                                }
                            })}
                        }
                    })
                }}
            </div>
                }
                .into_any()
            }
            {
                view! {
            <ConfirmDialog
                open=confirm_delete
                title="Delete this session?"
                body="This permanently deletes the session and its history from the store."
                confirm_label="Delete"
                destructive=true
                on_confirm=move || delete_confirmed.set(true)
            />
                }
                .into_any()
            }
        </div>
    }
        .into_any()
}

#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
fn SessionRowView(
    session: SessionRow,
    /// Pre-built `?session=` link — the page appends the live filter
    /// params so row clicks keep the filter bar in the URL.
    href: String,
    /// How many sub-agent sessions name this row as parent — renders
    /// as a `↳ N` meta marker.
    children: usize,
    queued: usize,
    failed: usize,
    selected: Signal<bool>,
    /// Page-level menu state — the row writes `(id, agent, x, y)` on
    /// `contextmenu` and the single menu instance renders against it.
    menu: RwSignal<MenuTarget>,
    /// Page-level: the id whose title is swapped for a rename input.
    renaming: RwSignal<Option<String>>,
    rename_draft: RwSignal<String>,
    /// Runs after a mutation lands — `sessions.refetch()`.
    on_changed: impl Fn() + Send + Sync + Copy + 'static,
) -> impl IntoView {
    let title = if session.title.trim().is_empty() {
        "Untitled session".to_string()
    } else {
        session.title.clone()
    };
    let is_sub = session.parent_session_id.is_some();
    let iso = session.updated_at.clone();
    let row_cls = move || {
        if selected.get() {
            "block rounded-md bg-accent px-2 py-2 transition-colors"
        } else {
            "block rounded-md px-2 py-2 transition-colors hover:bg-accent/60"
        }
    };
    let row_id = session.id.clone();
    let menu_id = session.id.clone();
    let menu_agent = non_empty(&session.agent);
    let dots_id = session.id.clone();
    let dots_agent = menu_agent.clone();
    let expanded_id = session.id.clone();
    let orig_title = session.title.clone();
    let commit_id = session.id.clone();
    let commit_agent = menu_agent.clone();
    let rename_ref = NodeRef::<leptos::html::Input>::new();
    let toast = use_toast();

    // Focus + select the inline rename input once it mounts.
    #[cfg(feature = "hydrate")]
    {
        let focus_id = session.id.clone();
        Effect::new(move |_| {
            if renaming.get().as_deref() == Some(focus_id.as_str())
                && let Some(el) = rename_ref.get()
            {
                let _ = el.focus();
                el.select();
            }
        });
    }

    view! {
        <li
            class="group relative"
            on:contextmenu=move |ev| {
                ev.prevent_default();
                menu.set(Some((
                    menu_id.clone(),
                    menu_agent.clone(),
                    f64::from(ev.client_x()),
                    f64::from(ev.client_y()),
                )));
            }
        >
            <A
                href=href
                attr:class=row_cls
                attr:aria-current=move || selected.get().then_some("true")
            >
                <span class="flex items-center justify-between gap-2">
                    {move || {
                        if renaming.get().as_deref() == Some(row_id.as_str()) {
                            // Per-render clones for the `move` handlers
                            // below — capturing the outer `String`s
                            // directly would make this closure `FnOnce`.
                            let orig = orig_title.clone();
                            let cid = commit_id.clone();
                            let cagent = commit_agent.clone();
                            view! {
                                // `prevent_default` on click keeps the
                                // anchor from navigating while editing;
                                // keydown stops propagation so the page
                                // hotkeys (Escape/n/arrows) stay out.
                                <input
                                    node_ref=rename_ref
                                    type="text"
                                    maxlength="200"
                                    class="h-6 w-full min-w-0 rounded-sm border border-input bg-transparent px-1 text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring"
                                    prop:value=move || rename_draft.get()
                                    on:input=move |ev| {
                                        rename_draft.set(event_target_value(&ev));
                                    }
                                    on:click=move |ev| ev.prevent_default()
                                    on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                        match ev.key().as_str() {
                                            "Enter" => {
                                                ev.prevent_default();
                                                ev.stop_propagation();
                                                renaming.set(None);
                                                let new_title =
                                                    rename_draft.get_untracked().trim().to_string();
                                                if new_title.is_empty()
                                                    || new_title == orig.trim()
                                                {
                                                    return;
                                                }
                                                let id = cid.clone();
                                                let agent = cagent.clone();
                                                leptos::task::spawn_local(async move {
                                                    if toast
                                                        .outcome(
                                                            rename_session(id, agent, new_title)
                                                                .await,
                                                            "Renamed",
                                                        )
                                                        .is_some()
                                                    {
                                                        on_changed();
                                                    }
                                                });
                                            }
                                            "Escape" => {
                                                ev.prevent_default();
                                                ev.stop_propagation();
                                                renaming.set(None);
                                            }
                                            _ => {}
                                        }
                                    }
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <span class="flex min-w-0 items-center gap-1 text-sm font-medium">
                                    // Marks sub-agent sessions — the
                                    // parent id points at the session
                                    // that spawned them.
                                    {is_sub.then(|| {
                                        view! {
                                            <span
                                                class="inline-flex shrink-0 text-muted-foreground"
                                                title="Sub-agent session"
                                                data-name="SubAgentMark"
                                            >
                                                <Icon name="corner-down-right" class="size-3"/>
                                            </span>
                                        }
                                    })}
                                    <span class="min-w-0 truncate">{title.clone()}</span>
                                </span>
                            }
                                .into_any()
                        }
                    }}
                    <span class="flex shrink-0 flex-wrap items-center justify-end gap-1">
                        {session
                            .node
                            .clone()
                            .map(|n| view! { <Badge variant=BadgeVariant::Info>{n}</Badge> })}
                        {session
                            .busy
                            .then(|| view! { <Badge variant=BadgeVariant::Warning>"busy"</Badge> })}
                        {session.locked.then(|| {
                            view! {
                                <Badge variant=BadgeVariant::Warning>
                                    <Icon name="lock" class="size-2.5"/>
                                    "locked"
                                </Badge>
                            }
                        })}
                        {(queued > 0).then(|| {
                            let label = if queued > 1 {
                                format!("queued ×{queued}")
                            } else {
                                "queued".to_string()
                            };
                            view! { <Badge variant=BadgeVariant::Default>{label}</Badge> }
                        })}
                        {(failed > 0).then(|| {
                            let label = if failed > 1 {
                                format!("failed ×{failed}")
                            } else {
                                "failed".to_string()
                            };
                            view! { <Badge variant=BadgeVariant::Destructive>{label}</Badge> }
                        })}
                        {session.pinned.then(|| {
                            view! {
                                <Badge variant=BadgeVariant::Success>
                                    <Icon name="pin" class="size-2.5"/>
                                    "pinned"
                                </Badge>
                            }
                        })}
                        {session.archived.then(|| {
                            view! { <Badge variant=BadgeVariant::Muted>"archived"</Badge> }
                        })}
                    </span>
                </span>
                <span class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                    <span class="shrink-0 rounded-sm border border-border bg-muted px-1.5 py-px font-mono text-[11px] text-info">
                        {session.agent.clone()}
                    </span>
                    <span class="min-w-0 truncate [direction:rtl] text-left">
                        {session.cwd.clone()}
                    </span>
                    {(children > 0).then(|| {
                        view! {
                            <span
                                class="inline-flex shrink-0 items-center gap-0.5"
                                title="Sub-agent sessions"
                                data-name="ChildCount"
                            >
                                <Icon name="corner-down-right" class="size-3"/>
                                {children}
                            </span>
                        }
                    })}
                    <span class="ml-auto shrink-0">
                        <RelativeTime iso=iso/>
                    </span>
                </span>
            </A>
            // The ellipsis — hover-reveal shortcut to the SAME menu
            // `contextmenu` opens (contextmenu never fires on touch;
            // `[@media(hover:none)]` keeps it visible there). Pointer
            // coords feed the shared `menu_for` signal; `menu_anchor`
            // falls back to the button's corner on keyboard-opened
            // clicks (whose clientX/Y are 0,0).
            <button
                type="button"
                aria-label="Session actions"
                aria-haspopup="menu"
                aria-controls="sepia-row-menu"
                aria-expanded=move || {
                    menu.read()
                        .as_ref()
                        .is_some_and(|(id, _, _, _)| *id == expanded_id)
                        .to_string()
                }
                data-name="RowMenuButton"
                class="absolute right-1.5 top-1.5 z-10 inline-flex size-6 items-center justify-center rounded-md border bg-popover text-muted-foreground opacity-0 shadow-sm transition-opacity hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 [@media(hover:none)]:opacity-100"
                on:click=move |ev| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    let (x, y) = menu_anchor(&ev);
                    menu.set(Some((
                        dots_id.clone(),
                        dots_agent.clone(),
                        x,
                        y,
                    )));
                }
            >
                <Icon name="ellipsis-vertical" class="size-3.5"/>
            </button>
        </li>
    }
}

/// Anchor point for the row `⋯` button's menu — pointer coords when
/// the click carried them; a keyboard-opened click reports (0,0), so
/// the button's bottom-right corner anchors the menu instead of the
/// viewport's top-left.
#[cfg(feature = "hydrate")]
fn menu_anchor(ev: &leptos::ev::MouseEvent) -> (f64, f64) {
    use wasm_bindgen::JsCast;
    let (x, y) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
    if x > 0.0 || y > 0.0 {
        return (x, y);
    }
    ev.current_target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
        .map_or((x, y), |el| {
            let r = el.get_bounding_client_rect();
            (r.right(), r.bottom())
        })
}

/// SSR counterpart — menus never open server-side; the signature just
/// keeps the `on:click` handler compiling.
#[cfg(not(feature = "hydrate"))]
fn menu_anchor(ev: &leptos::ev::MouseEvent) -> (f64, f64) {
    (f64::from(ev.client_x()), f64::from(ev.client_y()))
}

/// Every `[role=menuitem]` inside a menu container, DOM order — the
/// index space `active` and `aria-activedescendant` count. Disabled
/// items stay in the list (stable positional ids); `menu_enabled`
/// marks the slots the pure math may land on.
#[cfg(feature = "hydrate")]
fn menu_items(menu: &web_sys::HtmlElement) -> Vec<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;
    let Ok(list) = menu.query_selector_all("[role=menuitem]") else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

/// Enabled mask over `items` — `disabled` or `aria-disabled="true"`
/// opts out of roving, activation, and typeahead.
#[cfg(feature = "hydrate")]
fn menu_enabled(items: &[web_sys::HtmlElement]) -> Vec<bool> {
    items
        .iter()
        .map(|el| {
            !el.has_attribute("disabled")
                && el.get_attribute("aria-disabled").as_deref() != Some("true")
        })
        .collect()
}

/// Focus bookkeeping for one `role=menu` popup — the menu sibling of
/// `track_overlay_focus`, minus the Tab trap (menus dismiss on Tab,
/// they don't cycle). On open: remember the focused element (the
/// return-focus target), drop `active` on the first enabled item, and
/// focus the container that owns `aria-activedescendant` — deferred a
/// tick so the `hidden` class removal lands first. On close: hand
/// focus back to the opener *only* when it's still inside the menu —
/// item clicks that grabbed focus elsewhere first (rename input, cwd
/// field, a dialog) keep it; outside clicks arrive with focus already
/// gone. `open` reads the menu's target signal — it *tracks* the
/// signal, so re-anchoring to another row while open re-runs the
/// effect even though the bool itself holds.
#[cfg(feature = "hydrate")]
fn track_menu_focus(
    open: impl Fn() -> bool + Copy + 'static,
    menu: NodeRef<leptos::html::Div>,
    active: RwSignal<usize>,
) {
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    let prev = Rc::new(RefCell::new(None::<web_sys::HtmlElement>));
    Effect::new(move |_| {
        if open() {
            // Capture the opener only on a closed→open transition —
            // re-anchoring to another target while open mustn't
            // record the focused menu itself.
            if prev.borrow().is_none() {
                *prev.borrow_mut() = document()
                    .active_element()
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
            }
            let cb = Closure::<dyn FnMut()>::once(move || {
                // Bail if the menu closed again before the tick —
                // focusing a hidden element strands focus on <body>.
                if !open() {
                    return;
                }
                if let Some(el) = menu.get() {
                    let items = menu_items(&el);
                    let enabled = menu_enabled(&items);
                    if let Some(first) =
                        sepia_web_core::menu::edge(&enabled, sepia_web_core::menu::Edge::First)
                    {
                        active.set(first);
                    }
                    let _ = el.focus();
                }
            });
            let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                0,
            );
            cb.forget();
        } else {
            let restore = prev.borrow_mut().take();
            if let (Some(to), Some(el)) = (restore, menu.get()) {
                let inside = document()
                    .active_element()
                    .is_some_and(|a| el.contains(Some(a.unchecked_ref::<web_sys::Node>())));
                if inside {
                    let _ = to.focus();
                }
            }
        }
    });
}

/// Shared keydown dispatch for the page's `role=menu` popups (row +
/// group-header): arrows/Home/End move `active` (the
/// `aria-activedescendant` index, skipping disabled items), Enter/
/// Space clicks it, Escape closes (the `track_menu_focus` effect
/// returns focus to the opener), Tab closes without trapping, and a
/// bare printable char typeaheads. Pure math lives in
/// `sepia_web_core::menu`.
#[cfg(feature = "hydrate")]
fn menu_keydown(
    ev: &leptos::ev::KeyboardEvent,
    menu: NodeRef<leptos::html::Div>,
    active: RwSignal<usize>,
    close: impl Fn(),
) {
    use sepia_web_core::menu::{self, MenuKey};
    // Modifier chords belong to the page — ⌘K still opens the
    // palette over a menu rather than feeding typeahead.
    if ev.ctrl_key() || ev.meta_key() || ev.alt_key() {
        return;
    }
    match menu::classify(&ev.key()) {
        MenuKey::Ignore => {}
        // Tab — close and let the press through; menus don't trap.
        MenuKey::Dismiss => close(),
        // Escape — close; `track_menu_focus` returns focus.
        MenuKey::Close => {
            ev.prevent_default();
            ev.stop_propagation();
            close();
        }
        MenuKey::Move(dir) => {
            ev.prevent_default();
            ev.stop_propagation();
            let Some(el) = menu.get() else { return };
            let enabled = menu_enabled(&menu_items(&el));
            if let Some(i) = menu::move_active(&enabled, Some(active.get_untracked()), dir) {
                active.set(i);
            }
        }
        MenuKey::Jump(which) => {
            ev.prevent_default();
            ev.stop_propagation();
            let Some(el) = menu.get() else { return };
            let enabled = menu_enabled(&menu_items(&el));
            if let Some(i) = menu::edge(&enabled, which) {
                active.set(i);
            }
        }
        MenuKey::Type(c) => {
            ev.prevent_default();
            ev.stop_propagation();
            let Some(el) = menu.get() else { return };
            let items = menu_items(&el);
            let enabled = menu_enabled(&items);
            let labels: Vec<String> = items
                .iter()
                .map(|el| el.text_content().unwrap_or_default())
                .collect();
            if let Some(i) = menu::typeahead(&labels, &enabled, c, Some(active.get_untracked())) {
                active.set(i);
            }
        }
        MenuKey::Activate => {
            ev.prevent_default();
            ev.stop_propagation();
            let Some(el) = menu.get() else { return };
            let items = menu_items(&el);
            let enabled = menu_enabled(&items);
            let cur = active.get_untracked();
            if enabled.get(cur) == Some(&true) {
                items[cur].click();
            }
        }
    }
}

/// SSR stub — menus never open server-side; the signature keeps the
/// `on:keydown` handlers compiling.
#[cfg(not(feature = "hydrate"))]
fn menu_keydown(
    _ev: &leptos::ev::KeyboardEvent,
    _menu: NodeRef<leptos::html::Div>,
    _active: RwSignal<usize>,
    _close: impl Fn(),
) {
}

/// One `role=menuitem` in the page's context menus — a plain button
/// plus the `aria-activedescendant` bookkeeping: positional `id`
/// (what the container's attribute points at), `index` inside the
/// menu (what `active` counts), the `data-active` highlight, and a
/// `mouseenter` that keeps the two in sync under the pointer.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
fn CtxMenuItem(
    /// DOM id the menu's `aria-activedescendant` references.
    #[prop(into)]
    id: String,
    /// Position within the menu — compared against `active`.
    index: usize,
    /// The menu's active-descendant index.
    active: RwSignal<usize>,
    #[prop(optional)] destructive: bool,
    /// Runs on click — the item's action; closes the menu itself.
    on_click: Box<dyn Fn() + Send + Sync + 'static>,
    children: Children,
) -> impl IntoView {
    let cls = if destructive {
        MENU_ITEM_DESTRUCTIVE_CLS
    } else {
        MENU_ITEM_CLS
    };
    view! {
        <button
            type="button"
            role="menuitem"
            id=id
            class=move || {
                if active.get() == index {
                    if destructive {
                        format!("{cls} bg-destructive/10")
                    } else {
                        format!("{cls} bg-accent")
                    }
                } else {
                    cls.to_string()
                }
            }
            data-active=move || (active.get() == index).then_some("")
            on:mouseenter=move |_| active.set(index)
            on:click=move |_| on_click()
        >
            {children()}
        </button>
    }
}

/// Agent multi-select — a `<details>` dropdown of checkboxes over the
/// resolved list's distinct agents. Empty selection means "all
/// agents"; the trigger shows the pick count. Same native-details
/// pattern as `components::Dropdown`, bespoke because the label is
/// reactive and the items stay open while toggled.
#[component]
fn AgentFilter(
    /// The last resolved session list — options derive from it.
    rows: RwSignal<Vec<SessionRow>>,
    /// Selected agent ids (empty = all).
    selected: RwSignal<BTreeSet<String>>,
) -> impl IntoView {
    view! {
        <details class="group relative flex-1" data-name="AgentFilter">
            <summary class="list-none cursor-pointer select-none [&::-webkit-details-marker]:hidden">
                <span class=format!("{SELECT_CLASS} items-center justify-between")>
                    {move || {
                        let n = selected.read().len();
                        if n == 0 {
                            "all agents".to_string()
                        } else {
                            format!("{n} agent{}", if n == 1 { "" } else { "s" })
                        }
                    }}
                    <Icon
                        name="chevron-down"
                        class="size-3 opacity-60 transition-transform group-open:rotate-180"
                    />
                </span>
            </summary>
            // open: sibling backdrop swallows the outside click
            <span
                class="fixed inset-0 z-40 hidden cursor-default group-open:block"
                onclick="this.parentElement.removeAttribute('open')"
            ></span>
            <div class="absolute inset-x-0 z-50 mt-1 max-h-56 overflow-auto rounded-3xl bg-popover p-1.5 text-sm text-popover-foreground shadow-lg ring-1 ring-border">
                <button
                    type="button"
                    class=MENU_ITEM_CLS
                    on:click=move |_| selected.set(BTreeSet::new())
                >
                    "All agents"
                </button>
                {move || {
                    agent_options(&rows.read())
                        .into_iter()
                        .map(|a| {
                            let for_check = a.clone();
                            let for_toggle = a.clone();
                            view! {
                                <label class="flex w-full cursor-pointer items-center gap-2 rounded-2xl px-3 py-2 hover:bg-accent">
                                    <input
                                        type="checkbox"
                                        class="size-3.5 shrink-0 accent-primary"
                                        prop:checked={
                                            let a = for_check.clone();
                                            move || selected.read().contains(&a)
                                        }
                                        on:change={
                                            let a = for_toggle.clone();
                                            move |ev| {
                                                let on = event_target_checked(&ev);
                                                let a = a.clone();
                                                selected.update(|s| {
                                                    if on {
                                                        s.insert(a.clone());
                                                    } else {
                                                        s.remove(&a);
                                                    }
                                                });
                                            }
                                        }
                                    />
                                    <span class="min-w-0 truncate">{a}</span>
                                </label>
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>
        </details>
    }
}

/// `POST /api/sessions` `{cwd, agent?, title?, model?}`. The agent
/// select encodes `node|agent` in the option value so a multi-node hub
/// routes the create to the node advertising that agent; a bare agent
/// id (or the empty "default") goes to the primary. `cwd_ref` exposes
/// the working-directory input so the `n` hotkey can focus it; `cwd`
/// is page-owned so the group menu's "New session here" can prefill.
#[component]
fn NewSessionForm(
    on_created: impl Fn() + 'static + Send + Sync + Copy,
    cwd_ref: NodeRef<leptos::html::Input>,
    cwd: RwSignal<String>,
) -> impl IntoView {
    // Filled post-hydration — a Resource here resolves during SSR
    // differently than hydrate (tachys option-vs-comment mismatch).
    let agents = RwSignal::new(Vec::<AgentDto>::new());
    #[cfg(feature = "hydrate")]
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(list) = list_agents().await {
            agents.set(list);
        }
    });
    let navigate = use_navigate();
    let title = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let agent_sel = RwSignal::new(String::new());
    let creating = RwSignal::new(false);
    let form_error: RwSignal<Option<String>> = RwSignal::new(None);

    // Cwd autocomplete. `dir_results` is `(fetched parent, subdirs)` —
    // keyed by parent so a stale response can't feed a newer typed
    // value. Empty on SSR and first hydration alike; the dropdown is
    // always-rendered markup toggled only by class.
    let dir_results = RwSignal::new((String::new(), Vec::<String>::new()));
    let suggest_open = RwSignal::new(false);
    let highlight = RwSignal::new(0usize);
    let suggestions = Memo::new(move |_| {
        let (parent, dirs) = dir_results.get();
        let typed = cwd.get();
        match path_parts(&typed) {
            Some((p, _)) if p == parent => complete_path(&typed, &dirs),
            _ => Vec::new(),
        }
    });

    // Fetch the children of the typed path's parent (no-op for
    // non-absolute input). Routed to the node the `node|agent` select
    // encodes, like `create` below.
    let fetch_dirs = move |typed: &str| {
        let Some((parent, _)) = path_parts(typed) else {
            dir_results.set((String::new(), Vec::new()));
            return;
        };
        let node = agent_sel
            .get_untracked()
            .split_once('|')
            .map(|(n, _)| n.to_string());
        leptos::task::spawn_local(async move {
            if let Ok(dirs) = fs_dirs(parent.clone(), node).await {
                dir_results.set((parent, dirs));
            }
        });
    };

    // Complete the typed path to `dir` + a trailing `/`, then fetch its
    // children so the dropdown drills straight into the next level.
    let pick_dir = move |dir: String| {
        let next = format!("{}/", dir.trim_end_matches('/'));
        cwd.set(next.clone());
        highlight.set(0);
        suggest_open.set(true);
        fetch_dirs(&next);
    };

    let create = move || {
        let cwd_value = cwd.get().trim().to_string();
        if cwd_value.is_empty() || creating.get() {
            return;
        }
        creating.set(true);
        form_error.set(None);
        let sel = agent_sel.get();
        let (node, agent_id) = match sel.split_once('|') {
            Some((n, a)) => (Some(n.to_string()), non_empty(a)),
            None => (None, non_empty(&sel)),
        };
        let title = non_empty(&title.get());
        let model = non_empty(&model.get());
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            match create_session(cwd_value, agent_id, title, model, node).await {
                Ok(res) => {
                    on_created();
                    let href = if res.agent_id.is_empty() {
                        format!("/?session={}", res.id)
                    } else {
                        format!("/?session={}&agent={}", res.id, res.agent_id)
                    };
                    navigate(&href, NavigateOptions::default());
                }
                Err(e) => {
                    form_error.set(Some(e.to_string()));
                    creating.set(false);
                }
            }
        });
    };

    view! {
        <form
            class="flex flex-col gap-2"
            on:submit=move |ev| {
                ev.prevent_default();
                create();
            }
        >
            <div class="relative">
                // `{..}` so `node_ref` lands as a spread attr on the
                // rendered `<input>` (component props can't take it).
                <Input
                    {..}
                    node_ref=cwd_ref
                    attr:r#type="text"
                    attr:placeholder="Working directory (required)…"
                    attr:autocomplete="off"
                    attr:spellcheck="false"
                    attr:role="combobox"
                    attr:aria-controls="cwd-suggest"
                    attr:aria-expanded=move || {
                        (suggest_open.get() && !suggestions.read().is_empty()).to_string()
                    }
                    attr:aria-activedescendant=move || {
                        (suggest_open.get() && !suggestions.read().is_empty())
                            .then(|| format!("cwd-suggest-{}", highlight.get()))
                    }
                    prop:value=move || cwd.get()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        cwd.set(v.clone());
                        highlight.set(0);
                        suggest_open.set(true);
                        fetch_dirs(&v);
                    }
                    on:focus=move |_| {
                        suggest_open.set(true);
                        fetch_dirs(&cwd.get_untracked());
                    }
                    on:blur=move |_| suggest_open.set(false)
                    on:keydown=move |ev| {
                        let count = suggestions.get_untracked().len();
                        let open = suggest_open.get_untracked() && count > 0;
                        match ev.key().as_str() {
                            // The document-level ArrowUp/Down + Escape
                            // hotkeys skip editable targets, so these
                            // never fight the session-cycling keys.
                            "ArrowDown" if open => {
                                ev.prevent_default();
                                highlight.update(|h| *h = (*h + 1) % count);
                            }
                            "ArrowUp" if open => {
                                ev.prevent_default();
                                highlight.update(|h| *h = (*h + count - 1) % count);
                            }
                            "Enter" if open => {
                                // Complete the highlighted dir instead
                                // of submitting the form.
                                ev.prevent_default();
                                let i = highlight.get_untracked().min(count - 1);
                                if let Some(dir) = suggestions.get_untracked().get(i).cloned() {
                                    pick_dir(dir);
                                }
                            }
                            "Escape" if open => {
                                // The global Escape handler would drop
                                // `?session=` — the dropdown owns it.
                                ev.stop_propagation();
                                suggest_open.set(false);
                            }
                            _ => {}
                        }
                    }
                />
                // Suggestion popover — always rendered so SSR and first
                // hydration agree; `hidden` until a `dir_results` entry
                // lands post-hydration.
                <ul
                    id="cwd-suggest"
                    role="listbox"
                    aria-label="Directory suggestions"
                    class=move || {
                        let base = "absolute inset-x-0 top-full z-50 mt-1 max-h-48 overflow-auto \
                                    rounded-md border bg-popover p-1 text-sm text-popover-foreground \
                                    shadow-md";
                        if suggest_open.get() && !suggestions.read().is_empty() {
                            base.to_string()
                        } else {
                            format!("{base} hidden")
                        }
                    }
                >
                    {move || {
                        suggestions
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, dir)| {
                                let pick = dir.clone();
                                view! {
                                    <li
                                        id=format!("cwd-suggest-{i}")
                                        role="option"
                                        aria-selected=move || {
                                            (highlight.get() == i).to_string()
                                        }
                                        class=move || {
                                            let base = "cursor-pointer truncate rounded-sm px-2 \
                                                        py-1 font-mono text-xs";
                                            if highlight.get() == i {
                                                format!("{base} bg-accent text-accent-foreground")
                                            } else {
                                                base.to_string()
                                            }
                                        }
                                        // `mousedown`'s default would
                                        // blur the input before `click`
                                        // lands — prevent it so the
                                        // pick keeps focus.
                                        on:mousedown=move |ev| ev.prevent_default()
                                        on:click=move |_| pick_dir(pick.clone())
                                    >
                                        {dir}
                                    </li>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </ul>
            </div>
            <div class="flex gap-2">
                <Input
                    attr:r#type="text"
                    attr:placeholder="Title (optional)…"
                    attr:maxlength="200"
                    class="flex-1"
                    prop:value=move || title.get()
                    on:input=move |ev| title.set(event_target_value(&ev))
                />
                <Input
                    attr:r#type="text"
                    attr:placeholder="Model…"
                    attr:maxlength="100"
                    class="w-28"
                    prop:value=move || model.get()
                    on:input=move |ev| model.set(event_target_value(&ev))
                />
            </div>
            <div class="flex gap-2">
                <select
                    class=format!("{SELECT_CLASS} flex-1")
                    prop:value=move || agent_sel.get()
                    on:change=move |ev| agent_sel.set(event_target_value(&ev))
                >
                    <option value="">"default agent"</option>
                    // SSR + hydrate both render empty initially — options
                    // land post-hydration when `agents` fills.
                    {move || {
                        agents
                            .get()
                            .into_iter()
                            .map(|a| {
                                let value = match &a.node {
                                    Some(n) if !n.is_empty() => format!("{n}|{}", a.id),
                                    _ => a.id.clone(),
                                };
                                let base = if a.label.trim().is_empty() {
                                    a.id.clone()
                                } else {
                                    a.label.clone()
                                };
                                let label = match &a.node {
                                    Some(n) if !n.is_empty() => format!("{base} · {n}"),
                                    _ => base,
                                };
                                view! { <option value=value>{label}</option> }
                            })
                            .collect::<Vec<_>>()
                    }}
                </select>
                <Button
                    button_type="submit"
                    disabled=Signal::derive(move || {
                        creating.get() || cwd.read().trim().is_empty()
                    })
                >
                    <Icon name="plus" class="size-3.5"/>
                    {move || if creating.get() { "…" } else { "New" }}
                </Button>
            </div>
            {move || {
                form_error
                    .get()
                    .map(|e| view! { <ErrorBanner message=e/> })
            }}
        </form>
    }
}

/// The import card's file `change` → read the pick as text and parse
/// it. Hydrate-only like the composer's attachment reads —
/// `FileReader` doesn't exist under SSR.
#[cfg(feature = "hydrate")]
fn read_import_file(
    ev: &web_sys::Event,
    picked: RwSignal<Option<(String, Value)>>,
    toast: ToastStore,
) {
    use wasm_bindgen::JsCast;
    let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    let Some(file) = input.files().and_then(|f| f.get(0)) else {
        return;
    };
    let name = file.name();
    let Ok(reader) = web_sys::FileReader::new() else {
        toast.error(format!("couldn't read {name}"));
        return;
    };
    // The `once` closure owns a clone of the reader — it frees itself
    // (and the reader) after firing.
    let done_reader = reader.clone();
    let on_done = wasm_bindgen::closure::Closure::once(move |_: web_sys::ProgressEvent| {
        match done_reader.result() {
            Ok(v) => match v
                .as_string()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            {
                Some(json) => picked.set(Some((name, json))),
                None => toast.error(format!("{name} isn't valid JSON")),
            },
            Err(_) => toast.error(format!("couldn't read {name}")),
        }
    });
    reader.set_onloadend(Some(on_done.as_ref().unchecked_ref()));
    let _ = reader.read_as_text(&file);
    // Reset so picking the same file twice still fires `change`.
    input.set_value("");
}

/// SSR counterpart — file APIs don't exist; the markup is the
/// behavior-free shell either way.
#[cfg(not(feature = "hydrate"))]
fn read_import_file(
    _ev: &leptos::ev::Event,
    _picked: RwSignal<Option<(String, Value)>>,
    _toast: ToastStore,
) {
}

/// "Import session…" — a ghost button under the new-session form that
/// opens a `Sheet`: file picker for a session export (the `{session}`
/// IR `GET /api/sessions/:id/export` produces), a target-store select
/// (`cline`/`devin` — the node's `ImportTarget`), an optional cwd
/// override, and the POST. On success the merged list refreshes and
/// the imported copy is selected.
#[component]
fn ImportCard(
    /// Runs after an import lands — `sessions.refetch()`.
    on_imported: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let open = RwSignal::new(false);
    // `(file name, parsed JSON)` — staged by the picker until Import.
    let picked: RwSignal<Option<(String, Value)>> = RwSignal::new(None);
    let target = RwSignal::new("devin".to_string());
    let cwd = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let file_ref = NodeRef::<leptos::html::Input>::new();
    let toast = use_toast();
    // `use_navigate`'s context is `!Send` on wasm — the same
    // `new_local` + SSR-stub dance the page-level navigate does.
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});

    let do_import = move || {
        let Some((_name, json)) = picked.get_untracked() else {
            return;
        };
        if busy.get() {
            return;
        }
        let cwd_opt = non_empty(&cwd.get_untracked());
        let body = match import_body(&json, &target.get_untracked(), cwd_opt.as_deref()) {
            Ok(b) => b,
            Err(e) => {
                toast.error(e);
                return;
            }
        };
        busy.set(true);
        leptos::task::spawn_local(async move {
            if let Some(res) = toast.outcome(import_session(body).await, "Session imported") {
                open.set(false);
                on_imported();
                navigate.with_value(|n| {
                    n(
                        &sepia_web_core::filter::session_href(&res.id, &res.agent),
                        NavigateOptions::default(),
                    );
                });
            }
            busy.set(false);
        });
    };

    view! {
        <div data-name="ImportCard" class="pt-2">
            <Button
                variant=ButtonVariant::Ghost
                size=ButtonSize::Sm
                class="w-full justify-start text-muted-foreground"
                on_click=Box::new(move || open.set(true))
            >
                "Import session…"
            </Button>
            <Sheet open=open side="right" class="w-96" label="Import session">
                <SheetHeader>
                    <SheetTitle>"Import session"</SheetTitle>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Icon
                        class="size-7"
                        on_click=Box::new(move || open.set(false))
                    >
                        <Icon name="x" class="size-4"/>
                    </Button>
                </SheetHeader>
                <SheetBody class="space-y-4">
                    <p class="text-xs text-muted-foreground">
                        "Import a session export (the JSON the details sheet's Export downloads) into a store on this node."
                    </p>
                    <div class="space-y-1.5">
                        <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                            "Export file"
                        </label>
                        <div class="flex items-center gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                on_click=Box::new(move || {
                                    if let Some(el) = file_ref.get() {
                                        el.click();
                                    }
                                })
                            >
                                "Choose file…"
                            </Button>
                            <span class="min-w-0 truncate text-xs text-muted-foreground">
                                {move || {
                                    picked.read().as_ref().map_or_else(
                                        || "no file chosen".to_string(),
                                        |(n, _)| n.clone(),
                                    )
                                }}
                            </span>
                        </div>
                        <input
                            node_ref=file_ref
                            type="file"
                            accept=".json,application/json"
                            class="hidden"
                            data-name="ImportFileInput"
                            on:change=move |ev| read_import_file(&ev, picked, toast)
                        />
                    </div>
                    <div class="space-y-1.5">
                        <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                            "Target store"
                        </label>
                        <select
                            class=SELECT_CLASS
                            prop:value=move || target.get()
                            on:change=move |ev| target.set(event_target_value(&ev))
                        >
                            {CONVERT_TARGETS
                                .iter()
                                .map(|(id, label)| {
                                    view! { <option value=*id>{*label}</option> }
                                })
                                .collect::<Vec<_>>()}
                        </select>
                    </div>
                    <div class="space-y-1.5">
                        <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                            "Working directory (optional)"
                        </label>
                        <Input
                            attr:r#type="text"
                            attr:placeholder="Keep the session's cwd"
                            prop:value=move || cwd.get()
                            on:input=move |ev| cwd.set(event_target_value(&ev))
                        />
                    </div>
                    <Button
                        disabled=Signal::derive(move || busy.get() || picked.read().is_none())
                        on_click=Box::new(do_import)
                    >
                        {move || if busy.get() { "Importing…" } else { "Import" }}
                    </Button>
                </SheetBody>
            </Sheet>
        </div>
    }
}

/// `<time datetime=…>` with a live relative label.
#[component]
pub fn RelativeTime(#[prop(into)] iso: String) -> impl IntoView {
    let stamp = iso.clone();
    let label = move || {
        let now = use_context::<Now>().map_or_else(crate::time::now_ms, |n| n.0.get());
        relative(&stamp, now)
    };
    view! {
        <time datetime=iso.clone() title=iso>
            {label}
        </time>
    }
}
