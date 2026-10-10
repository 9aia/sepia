//! `/` — master-detail home. The session list lives in a left column
//! (sidebar-style on `lg+`, full-width below); selecting a row sets
//! `?session=<id>&agent=<agent>` and the chat pane mounts
//! `SessionPanel` inline. `/sessions/:id` redirects here.
//!
//! SSR renders the first page; on the client the list refreshes when
//! `/api/events` says a session row changed, and on a 30s ticker
//! (relative timestamps).

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
#[cfg(feature = "hydrate")]
use sepia_web_core::filter::visible_keys;
use sepia_web_core::filter::{
    ALL_AGENTS, CollapsedGroups, SessionFilter, SessionRow, SortMode, StatusFilter, agent_options,
    group_open, session_href,
};
#[cfg(feature = "hydrate")]
use sepia_web_core::keymap::{self, KeyCtx, Mods, NavDir};
use sepia_web_core::path::{complete_path, non_empty, path_parts};

#[cfg(feature = "hydrate")]
use crate::api::list_agents;
use crate::api::{create_session, delete_session, fs_dirs, pin_session, rename_session};
use crate::app::Now;
#[cfg(feature = "hydrate")]
use crate::app::in_editable;
use crate::components::icons::Icon;
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ConfirmDialog, EmptyState, Input, SELECT_CLASS, Skeleton,
};
use crate::dto::AgentDto;
use crate::pages::SessionPanel;
use crate::time::relative;

/// Context-menu item styling — same look as `MenuItem`, minus the
/// `<details>` close hook (the row menu isn't a `<details>`).
const MENU_ITEM_CLS: &str =
    "flex w-full items-center rounded-sm px-2 py-1.5 text-sm hover:bg-accent cursor-pointer";
const MENU_ITEM_DESTRUCTIVE_CLS: &str = "flex w-full items-center rounded-sm px-2 py-1.5 text-sm \
                                        text-destructive hover:bg-destructive/10 cursor-pointer";

/// Context-menu state — `(session id, agent, clientX, clientY)`.
/// `None` while the menu is closed.
type MenuTarget = Option<(String, Option<String>, f64, f64)>;

#[component]
pub fn SessionListPage() -> impl IntoView {
    let client = crate::api::query_client();
    let sessions = client.resource(crate::api::sessions_scope, || ());
    let pending = client.resource(crate::api::pending_scope, || ());
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

    // Filter/sort/grouping state — all client-side. Defaults render the
    // identical DOM on SSR and first hydration; every divergence is
    // applied reactively afterwards.
    let filter_text = RwSignal::new(String::new());
    let sort_mode = RwSignal::new(SortMode::default());
    let status_filter = RwSignal::new(StatusFilter::default());
    let agent_filter = RwSignal::new(ALL_AGENTS.to_string());
    // `⌘B` hides the list column (desktop only — `lg:hidden`).
    let list_collapsed = RwSignal::new(false);
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
            match delete_session(id, agent).await {
                Ok(()) => {
                    sessions.refetch();
                    navigate.with_value(|n| n("/", NavigateOptions::default()));
                }
                Err(e) => toast.error(e.to_string()),
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
    // filtered, sorted, grouped, and with collapsed sections skipped —
    // so ArrowUp/ArrowDown can move the `?session=` selection.
    #[cfg(feature = "hydrate")]
    let visible_order = move || -> Vec<(String, String)> {
        let filter = SessionFilter {
            query: filter_text.get_untracked(),
            status: status_filter.get_untracked(),
            agent: agent_filter.get_untracked(),
            sort: sort_mode.get_untracked(),
        };
        let groups = filter.groups(all_sessions.get_untracked());
        visible_keys(&groups, &closed_groups.get_untracked(), filter.force_open())
    };

    // Refresh when the node feed reports a session/meta/project change.
    #[cfg(feature = "hydrate")]
    {
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            sessions.refetch();
            pending.refetch();
        }));
        crate::app::every_ms(30_000, move || pending.refetch());
        // Clicking anywhere outside the menu closes it (the right-click
        // that opens it precedes `contextmenu`, not a `click`, so it
        // can't immediately re-close).
        let _outside = leptos_use::on_click_outside(menu_ref, move |_| menu_for.set(None));
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
    // mapping is pure (`sepia_web_core::keymap::resolve_key`); this
    // listener owns the list-page actions — `ToggleHelp`/`CloseHelp`
    // belong to the shell's listener in `app.rs`.
    #[cfg(feature = "hydrate")]
    {
        let navigate = use_navigate();
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
                    menu_open: menu_for.get_untracked().is_some(),
                    ..KeyCtx::default()
                };
                let Some(action) = keymap::resolve_key(&ev.key(), mods, ctx) else {
                    return;
                };
                if action.prevent_default() {
                    ev.prevent_default();
                }
                match action {
                    // ⌘K / Ctrl-K — focus the filter input.
                    keymap::Action::FocusFilter => {
                        if let Some(el) = filter_input_ref.get() {
                            let _ = el.focus();
                        }
                    }
                    // ⌘B / Ctrl-B — toggle the list column.
                    keymap::Action::ToggleList => list_collapsed.update(|v| *v = !*v),
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
                    keymap::Action::CloseMenu => menu_for.set(None),
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
                    // Owned by the shell listener (`app.rs`).
                    keymap::Action::ToggleHelp | keymap::Action::CloseHelp => {}
                }
            },
        );
    }

    view! {
        <Title text="sessions — sepia"/>
        <div class="flex h-full min-h-0 w-full">
            // List column — a fixed sidebar on lg+, full page below that
            // (where it hides once a session is selected).
            {
                // `.into_any()` at each layout joint caps rustc's
                // view-type depth (the nested Suspense/Form tree
                // overflowed the query-depth limit in sepia-hub).
                view! {
            <div class=move || {
                let base = "min-w-0 flex-col overflow-y-auto lg:flex lg:w-96 lg:shrink-0 lg:border-r";
                // `⌘B` collapse is desktop-only (`lg:hidden`) — on small
                // screens the column stays usable.
                let collapse = if list_collapsed.get() { " lg:hidden" } else { "" };
                if selected().is_some() {
                    format!("hidden {base}{collapse}")
                } else {
                    format!("flex {base} flex-1{collapse}")
                }
            }>
                <div class="border-b p-3">
                    <NewSessionForm
                        cwd_ref=cwd_input_ref
                        on_created=move || sessions.refetch()
                    />
                </div>
                // Filter bar — client-side only. The agent options read
                // `all_sessions`, which is empty until the Suspend below
                // resolves, so SSR and first hydration emit the same
                // single "all agents" option (like the agent select in
                // NewSessionForm).
                <div class="border-b p-2">
                    <div class="flex flex-col gap-1.5">
                        // `{..}` marks everything after it as spread
                        // attrs on the component's root element —
                        // `node_ref` only works that way on components.
                        <Input
                            {..}
                            node_ref=filter_input_ref
                            attr:r#type="text"
                            attr:placeholder="Filter sessions… ⌘K"
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
                            <select
                                class=format!("{SELECT_CLASS} flex-1")
                                prop:value=move || agent_filter.get()
                                on:change=move |ev| agent_filter.set(event_target_value(&ev))
                            >
                                <option value="all">"all agents"</option>
                                {move || {
                                    agent_options(&all_sessions.read())
                                        .into_iter()
                                        .map(|a| {
                                            let label = a.clone();
                                            view! { <option value=a>{label}</option> }
                                        })
                                        .collect::<Vec<_>>()
                                }}
                            </select>
                        </div>
                    </div>
                </div>
                <div class="flex-1 overflow-y-auto p-2">
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
                                // A broken outbox read never sinks the list.
                                let pending_result = pending.await.unwrap_or_default();
                                match sessions.await {
                                    Err(e) => {
                                        all_sessions.set(Vec::new());
                                        view! {
                                            <p class="whitespace-pre-wrap p-2 font-mono text-sm text-destructive">
                                                {e.to_string()}
                                            </p>
                                        }
                                            .into_any()
                                    }
                                    Ok(list) if list.is_empty() => {
                                        all_sessions.set(Vec::new());
                                        view! {
                                            <p class="p-2 text-sm text-muted-foreground">
                                                "No sessions yet."
                                            </p>
                                        }
                                            .into_any()
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
                                        let total = list.len();
                                        view! {
                                            {move || {
                                                let filter = SessionFilter {
                                                    query: filter_text.get(),
                                                    status: status_filter.get(),
                                                    agent: agent_filter.get(),
                                                    sort: sort_mode.get(),
                                                };
                                                let groups = filter.groups(list.clone());
                                                let shown: usize =
                                                    groups.iter().map(|g| g.rows.len()).sum();
                                                // A text filter force-expands
                                                // every section.
                                                let force_open = filter.force_open();
                                                let heading = filter.heading(shown, total);
                                                let sections = groups
                                                    .into_iter()
                                                    .map(|group| {
                                                        let count = group.rows.len();
                                                        let label = group.label.clone();
                                                        // Class-driven collapse —
                                                        // the signal flips the
                                                        // `<ul>` class without
                                                        // re-rendering the rows.
                                                        let closed = Signal::derive({
                                                            let key = group.key.clone();
                                                            move || {
                                                                !group_open(
                                                                    &key,
                                                                    &closed_groups.read(),
                                                                    force_open,
                                                                )
                                                            }
                                                        });
                                                        let toggle_key = group.key.clone();
                                                        let rows = group
                                                            .rows
                                                            .into_iter()
                                                            .map(|s| {
                                                                let (queued, failed) = writes
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
                                                                view! {
                                                                    <SessionRowView
                                                                        session=s
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
                                                                    class="flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[11px] font-semibold uppercase tracking-wider text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                                                                    on:click=move |_| {
                                                                        closed_groups.update(|v| {
                                                                            v.toggle(&toggle_key);
                                                                        });
                                                                    }
                                                                >
                                                                    <span class="inline-block w-3 shrink-0">
                                                                        {move || {
                                                                            if closed.get() { "▸" } else { "▾" }
                                                                        }}
                                                                    </span>
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
            </div>
                        }
                        .into_any()
                    }
            // Chat pane — fills the rest on lg+; takes over the whole
            // screen below lg once a session is selected.
            {
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
                                    icon="◈"
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
            // Row context menu — a single fixed-position instance moved
            // to the pointer on `contextmenu`. Always rendered (`hidden`
            // while closed) so SSR and hydrate emit identical DOM;
            // closes on outside click and Escape.
            {
                view! {
            <div
                node_ref=menu_ref
                role="menu"
                class=move || {
                    if menu_for.read().is_some() {
                        "fixed z-50 w-44 rounded-md border bg-popover p-1 shadow-lg"
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
            >
                {move || {
                    menu_for.get().map(|(id, agent, _x, _y)| {
                        let open_href = session_href(&id, agent.as_deref().unwrap_or_default());
                        let detail_href = match &agent {
                            Some(a) => format!("/sessions/{id}?agent={a}"),
                            None => format!("/sessions/{id}"),
                        };
                        let pinned_now = all_sessions
                            .read()
                            .iter()
                            .find(|s| s.id == id)
                            .is_some_and(|s| s.pinned);
                        view! {
                            <button
                                type="button"
                                role="menuitem"
                                class=MENU_ITEM_CLS
                                on:click={
                                    let href = open_href.clone();
                                    move |_| {
                                        menu_for.set(None);
                                        navigate.with_value(|n| {
                                            n(&href, NavigateOptions::default());
                                        });
                                    }
                                }
                            >
                                "Open"
                            </button>
                            <button
                                type="button"
                                role="menuitem"
                                class=MENU_ITEM_CLS
                                on:click={
                                    let id = id.clone();
                                    move |_| {
                                        menu_for.set(None);
                                        let draft = all_sessions
                                            .read()
                                            .iter()
                                            .find(|s| s.id == id)
                                            .map_or_else(String::new, |s| s.title.clone());
                                        rename_draft.set(draft);
                                        renaming.set(Some(id.clone()));
                                    }
                                }
                            >
                                "Rename…"
                            </button>
                            <button
                                type="button"
                                role="menuitem"
                                class=MENU_ITEM_CLS
                                on:click={
                                    let id = id.clone();
                                    let agent = agent.clone();
                                    move |_| {
                                        menu_for.set(None);
                                        let id = id.clone();
                                        let agent = agent.clone();
                                        leptos::task::spawn_local(async move {
                                            match pin_session(id, agent, !pinned_now).await {
                                                Ok(()) => sessions.refetch(),
                                                Err(e) => toast.error(e.to_string()),
                                            }
                                        });
                                    }
                                }
                            >
                                {if pinned_now { "Unpin" } else { "Pin" }}
                            </button>
                            <button
                                type="button"
                                role="menuitem"
                                class=MENU_ITEM_DESTRUCTIVE_CLS
                                on:click={
                                    let id = id.clone();
                                    let agent = agent.clone();
                                    move |_| {
                                        menu_for.set(None);
                                        pending_delete.set(Some((id.clone(), agent.clone())));
                                        confirm_delete.set(true);
                                    }
                                }
                            >
                                "Delete…"
                            </button>
                            <button
                                type="button"
                                role="menuitem"
                                class=MENU_ITEM_CLS
                                on:click={
                                    let href = detail_href.clone();
                                    move |_| {
                                        menu_for.set(None);
                                        navigate.with_value(|n| {
                                            n(&href, NavigateOptions::default());
                                        });
                                    }
                                }
                            >
                                "Details"
                            </button>
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
}

#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
fn SessionRowView(
    session: SessionRow,
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
    let href = session_href(&session.id, &session.agent);
    let iso = session.updated_at.clone();
    let row_cls = move || {
        if selected.get() {
            "block rounded-md bg-accent p-2.5 transition-colors"
        } else {
            "block rounded-md p-2.5 transition-colors hover:bg-accent/60"
        }
    };
    let row_id = session.id.clone();
    let menu_id = session.id.clone();
    let menu_agent = non_empty(&session.agent);
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
        <li on:contextmenu=move |ev| {
            ev.prevent_default();
            menu.set(Some((
                menu_id.clone(),
                menu_agent.clone(),
                f64::from(ev.client_x()),
                f64::from(ev.client_y()),
            )));
        }>
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
                                                    match rename_session(id, agent, new_title)
                                                        .await
                                                    {
                                                        Ok(()) => on_changed(),
                                                        Err(e) => toast.error(e.to_string()),
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
                                <span class="min-w-0 truncate text-sm font-medium">
                                    {title.clone()}
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
                            view! { <Badge variant=BadgeVariant::Warning>"locked"</Badge> }
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
                    </span>
                </span>
                <span class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                    <span class="shrink-0 rounded-sm border border-border bg-muted px-1.5 py-px font-mono text-[11px] text-info">
                        {session.agent.clone()}
                    </span>
                    <span class="min-w-0 truncate [direction:rtl] text-left">
                        {session.cwd.clone()}
                    </span>
                    <span class="ml-auto shrink-0">
                        <RelativeTime iso=iso/>
                    </span>
                </span>
            </A>
        </li>
    }
}

/// `POST /api/sessions` `{cwd, agent?, title?, model?}`. The agent
/// select encodes `node|agent` in the option value so a multi-node hub
/// routes the create to the node advertising that agent; a bare agent
/// id (or the empty "default") goes to the primary. `cwd_ref` exposes
/// the working-directory input so the `n` hotkey can focus it.
#[component]
fn NewSessionForm(
    on_created: impl Fn() + 'static + Send + Sync + Copy,
    cwd_ref: NodeRef<leptos::html::Input>,
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
    let cwd = RwSignal::new(String::new());
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
                <ul class=move || {
                    let base = "absolute inset-x-0 top-full z-50 mt-1 max-h-48 overflow-auto \
                                rounded-md border bg-popover p-1 text-sm text-popover-foreground \
                                shadow-md";
                    if suggest_open.get() && !suggestions.read().is_empty() {
                        base.to_string()
                    } else {
                        format!("{base} hidden")
                    }
                }>
                    {move || {
                        suggestions
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, dir)| {
                                let pick = dir.clone();
                                view! {
                                    <li
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
                    .map(|e| view! { <p class="text-sm text-destructive">{e}</p> })
            }}
        </form>
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
