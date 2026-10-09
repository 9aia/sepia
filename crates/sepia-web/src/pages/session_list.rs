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
use crate::api::list_agents;
use crate::api::{create_session, list_sessions, pending_writes};
use crate::app::Now;
use crate::components::{Badge, BadgeVariant, Button, EmptyState, Input, SELECT_CLASS, Skeleton};
use crate::dto::AgentDto;
use crate::dto::SessionSummaryDto;
use crate::pages::SessionPanel;
use crate::time::relative;

#[component]
pub fn SessionListPage() -> impl IntoView {
    let sessions = Resource::new(|| (), |()| list_sessions());
    let pending = Resource::new(|| (), |()| pending_writes());
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
    let agent_filter = RwSignal::new("all".to_string());
    // `⌘B` hides the list column (desktop only — `lg:hidden`).
    let list_collapsed = RwSignal::new(false);
    // Cwds of collapsed project groups. Starts empty (all sections
    // open) on both targets; a post-hydration Effect adopts the set
    // persisted in localStorage, so collapse is purely class-driven.
    let closed_groups = RwSignal::new(Vec::<String>::new());
    // The last resolved list — feeds the agent select (whose options
    // render empty on SSR + first hydration either way) and keyboard
    // navigation, which live outside the Suspend view tree.
    let all_sessions = RwSignal::new(Vec::<SessionSummaryDto>::new());
    // `n` focuses the new-session cwd input; `⌘K`/`Escape` the filter.
    let cwd_input_ref = NodeRef::<leptos::html::Input>::new();
    let filter_input_ref = NodeRef::<leptos::html::Input>::new();

    // Ordered `(id, agent)` of the rows currently on screen —
    // filtered, sorted, grouped, and with collapsed sections skipped —
    // so ArrowUp/ArrowDown can move the `?session=` selection.
    #[cfg(feature = "hydrate")]
    let visible_order = move || -> Vec<(String, String)> {
        let query = filter_text.get_untracked();
        let filtered = apply_filters(
            all_sessions.get_untracked(),
            &query,
            status_filter.get_untracked(),
            &agent_filter.get_untracked(),
            sort_mode.get_untracked(),
        );
        // Groups are force-expanded while a text filter is active.
        let force_open = !query.trim().is_empty();
        let closed = closed_groups.get_untracked();
        group_by_cwd(filtered)
            .into_iter()
            .filter(|(cwd, _)| force_open || !closed.contains(cwd))
            .flat_map(|(_, items)| items)
            .map(|s| (s.id, s.agent))
            .collect()
    };

    // Refresh when the node feed reports a session/meta/project change.
    #[cfg(feature = "hydrate")]
    {
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            sessions.refetch();
            pending.refetch();
        }));
        crate::app::every_ms(30_000, move || pending.refetch());
    }

    // Persist collapsed groups in localStorage. The Effect's first run
    // adopts the stored set (post-hydration only — SSR stays open);
    // subsequent runs write toggles back.
    #[cfg(feature = "hydrate")]
    {
        let (stored, set_stored, _clear) = leptos_use::storage::use_local_storage::<
            Vec<String>,
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

    // Global hotkeys — document keydown, client only.
    #[cfg(feature = "hydrate")]
    {
        let navigate = use_navigate();
        let _stop = leptos_use::use_event_listener(
            leptos::prelude::document(),
            leptos::ev::keydown,
            move |ev: web_sys::KeyboardEvent| {
                let mod_key = ev.meta_key() || ev.ctrl_key();
                let key = ev.key();
                if mod_key && key.eq_ignore_ascii_case("k") {
                    // ⌘K / Ctrl-K — focus the filter input.
                    ev.prevent_default();
                    if let Some(el) = filter_input_ref.get() {
                        let _ = el.focus();
                    }
                } else if mod_key && key.eq_ignore_ascii_case("b") {
                    // ⌘B / Ctrl-B — toggle the list column.
                    ev.prevent_default();
                    list_collapsed.update(|v| *v = !*v);
                } else if key == "Escape" {
                    // Escape — clear + blur the filter when focused,
                    // otherwise drop the `?session=` selection.
                    let filter_active = filter_input_ref
                        .get()
                        .zip(leptos::prelude::document().active_element())
                        .is_some_and(|(el, active)| {
                            use wasm_bindgen::JsCast;
                            active == *el.unchecked_ref::<web_sys::Element>()
                        });
                    if filter_active {
                        filter_text.set(String::new());
                        if let Some(el) = filter_input_ref.get() {
                            let _ = el.blur();
                        }
                    } else {
                        navigate("/", NavigateOptions::default());
                    }
                } else if !mod_key
                    && !ev.alt_key()
                    && !ev.shift_key()
                    && key.eq_ignore_ascii_case("n")
                    && !in_editable(&ev)
                {
                    // `n` — jump to the new-session cwd input.
                    ev.prevent_default();
                    if let Some(el) = cwd_input_ref.get() {
                        let _ = el.focus();
                    }
                } else if !mod_key
                    && !ev.alt_key()
                    && (key == "ArrowDown" || key == "ArrowUp")
                    && !in_editable(&ev)
                {
                    // Arrow keys — move `?session=` through the visible
                    // row order (wrapping at the ends).
                    let order = visible_order();
                    if order.is_empty() {
                        return;
                    }
                    let cur = selected_id.get_untracked();
                    let idx = cur
                        .as_deref()
                        .and_then(|id| order.iter().position(|(sid, _)| sid == id));
                    let next = match (key.as_str(), idx) {
                        ("ArrowDown", Some(i)) => (i + 1) % order.len(),
                        ("ArrowUp", Some(i)) => (i + order.len() - 1) % order.len(),
                        ("ArrowDown", None) => 0,
                        _ => order.len() - 1,
                    };
                    let (id, agent) = &order[next];
                    let href = if agent.is_empty() {
                        format!("/?session={id}")
                    } else {
                        format!("/?session={id}&agent={agent}")
                    };
                    navigate(&href, NavigateOptions::default());
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
                                    let mut agents: Vec<String> = all_sessions
                                        .read()
                                        .iter()
                                        .filter(|s| !s.agent.is_empty())
                                        .map(|s| s.agent.clone())
                                        .collect();
                                    agents.sort();
                                    agents.dedup();
                                    agents
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
                                                let query = filter_text.get();
                                                let status = status_filter.get();
                                                let agent = agent_filter.get();
                                                let sort = sort_mode.get();
                                                let filtered = apply_filters(
                                                    list.clone(),
                                                    &query,
                                                    status,
                                                    &agent,
                                                    sort,
                                                );
                                                let shown = filtered.len();
                                                let filtering = !query.trim().is_empty()
                                                    || status != StatusFilter::All
                                                    || agent != "all";
                                                // A text filter force-expands
                                                // every section.
                                                let force_open = !query.trim().is_empty();
                                                let heading = if filtering {
                                                    format!("{shown} of {total} sessions")
                                                } else {
                                                    format!("{total} sessions")
                                                };
                                                let sections = group_by_cwd(filtered)
                                                    .into_iter()
                                                    .map(|(cwd, items)| {
                                                        let count = items.len();
                                                        let label = cwd_label(&cwd);
                                                        let closed = Signal::derive({
                                                            let cwd = cwd.clone();
                                                            move || {
                                                                !force_open
                                                                    && closed_groups
                                                                        .read()
                                                                        .contains(&cwd)
                                                            }
                                                        });
                                                        let toggle_cwd = cwd.clone();
                                                        let rows = items
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
                                                                    <SessionRow
                                                                        session=s
                                                                        queued=queued
                                                                        failed=failed
                                                                        selected=is_selected
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
                                                                        let cwd = toggle_cwd.clone();
                                                                        closed_groups.update(|v| {
                                                                            if let Some(i) = v
                                                                                .iter()
                                                                                .position(|c| *c == cwd)
                                                                            {
                                                                                v.remove(i);
                                                                            } else {
                                                                                v.push(cwd);
                                                                            }
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
        </div>
    }
}

#[component]
fn SessionRow(
    session: SessionSummaryDto,
    queued: usize,
    failed: usize,
    selected: Signal<bool>,
) -> impl IntoView {
    let title = if session.title.trim().is_empty() {
        "Untitled session".to_string()
    } else {
        session.title.clone()
    };
    let href = match &session.agent {
        agent if agent.is_empty() => format!("/?session={}", session.id),
        agent => format!("/?session={}&agent={agent}", session.id),
    };
    let iso = session.updated_at.clone();
    let row_cls = move || {
        if selected.get() {
            "block rounded-md bg-accent p-2.5 transition-colors"
        } else {
            "block rounded-md p-2.5 transition-colors hover:bg-accent/60"
        }
    };
    view! {
        <li>
            <A
                href=href
                attr:class=row_cls
                attr:aria-current=move || selected.get().then_some("true")
            >
                <span class="flex items-center justify-between gap-2">
                    <span class="min-w-0 truncate text-sm font-medium">{title}</span>
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
                            view! { <Badge variant=BadgeVariant::Success>"pinned"</Badge> }
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
            // `{..}` so `node_ref` lands as a spread attr on the
            // rendered `<input>` (component props can't take it).
            <Input
                {..}
                node_ref=cwd_ref
                attr:r#type="text"
                attr:placeholder="Working directory (required)…"
                prop:value=move || cwd.get()
                on:input=move |ev| cwd.set(event_target_value(&ev))
            />
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

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

/// Sort order for the session list (filter-bar select values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SortMode {
    /// `updated_at` descending.
    #[default]
    Newest,
    /// `updated_at` ascending.
    Oldest,
    /// A–Z by title, newest first on ties.
    Title,
}

impl SortMode {
    fn parse(s: &str) -> Self {
        match s {
            "oldest" => Self::Oldest,
            "title" => Self::Title,
            _ => Self::Newest,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Title => "title",
        }
    }
}

/// Lock-state filter for the session list (filter-bar select values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StatusFilter {
    #[default]
    All,
    /// Unlocked sessions only.
    Free,
    /// Locked sessions only.
    Locked,
}

impl StatusFilter {
    fn parse(s: &str) -> Self {
        match s {
            "free" => Self::Free,
            "locked" => Self::Locked,
            _ => Self::All,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Free => "free",
            Self::Locked => "locked",
        }
    }
}

/// Apply the filter bar: case-insensitive substring match on title +
/// cwd, lock-state filter, agent filter, then the chosen sort.
/// `agent` is `"all"` or an exact agent id.
fn apply_filters(
    list: Vec<SessionSummaryDto>,
    query: &str,
    status: StatusFilter,
    agent: &str,
    sort: SortMode,
) -> Vec<SessionSummaryDto> {
    let q = query.trim().to_lowercase();
    let mut list: Vec<SessionSummaryDto> = list
        .into_iter()
        .filter(|s| {
            let query_ok = q.is_empty()
                || s.title.to_lowercase().contains(&q)
                || s.cwd.to_lowercase().contains(&q);
            let status_ok = match status {
                StatusFilter::All => true,
                StatusFilter::Free => !s.locked,
                StatusFilter::Locked => s.locked,
            };
            let agent_ok = agent == "all" || s.agent == agent;
            query_ok && status_ok && agent_ok
        })
        .collect();
    match sort {
        SortMode::Newest => list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        SortMode::Oldest => list.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
        SortMode::Title => list.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        }),
    }
    list
}

/// Group by `cwd`. Non-empty-cwd groups come first, ordered by the
/// most recent `updated_at` within the group; the empty-cwd
/// ("uncategorized") group is always last. Row order inside each
/// group is preserved.
fn group_by_cwd(list: Vec<SessionSummaryDto>) -> Vec<(String, Vec<SessionSummaryDto>)> {
    let mut map: BTreeMap<String, Vec<SessionSummaryDto>> = BTreeMap::new();
    for s in list {
        map.entry(s.cwd.clone()).or_default().push(s);
    }
    let (mut named, unnamed): (Vec<_>, Vec<_>) =
        map.into_iter().partition(|(cwd, _)| !cwd.is_empty());
    named.sort_by(|(_, a), (_, b)| newest_updated(b).cmp(newest_updated(a)));
    named.extend(unnamed);
    named
}

/// The newest `updated_at` in a group — RFC 3339 strings order
/// lexicographically.
fn newest_updated(items: &[SessionSummaryDto]) -> &str {
    items
        .iter()
        .map(|s| s.updated_at.as_str())
        .max()
        .unwrap_or_default()
}

/// Section label for a `cwd` — the last path segment, or
/// `"No project"` when the cwd is empty (or a bare root).
fn cwd_label(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    if base.is_empty() {
        "No project".to_string()
    } else {
        base.to_string()
    }
}

/// Is the keydown aimed at a text-entry element? input/textarea/
/// select/contenteditable swallow plain keys like `n` and the arrows.
#[cfg(feature = "hydrate")]
fn in_editable(ev: &web_sys::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    let Some(el) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    else {
        return false;
    };
    if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
        if html.is_content_editable() {
            return true;
        }
    }
    matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn s(
        id: &str,
        title: &str,
        cwd: &str,
        agent: &str,
        updated_at: &str,
        locked: bool,
    ) -> SessionSummaryDto {
        SessionSummaryDto {
            id: id.into(),
            title: title.into(),
            cwd: cwd.into(),
            agent: agent.into(),
            updated_at: updated_at.into(),
            locked,
            ..SessionSummaryDto::default()
        }
    }

    fn list() -> Vec<SessionSummaryDto> {
        vec![
            s(
                "a",
                "Fix Auth",
                "/home/u/app",
                "claude",
                "2026-10-08T10:00:00Z",
                false,
            ),
            s(
                "b",
                "docs",
                "/home/u/app",
                "cline",
                "2026-10-09T10:00:00Z",
                true,
            ),
            s(
                "c",
                "API work",
                "/var/www/site",
                "claude",
                "2026-10-07T10:00:00Z",
                false,
            ),
            s("d", "", "", "cursor", "2026-10-06T10:00:00Z", false),
        ]
    }

    fn ids(list: &[SessionSummaryDto]) -> Vec<&str> {
        list.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn query_matches_title_and_cwd_case_insensitively() {
        let out = apply_filters(list(), "AUTH", StatusFilter::All, "all", SortMode::Newest);
        assert_eq!(ids(&out), ["a"]);
        let out = apply_filters(list(), "site", StatusFilter::All, "all", SortMode::Newest);
        assert_eq!(ids(&out), ["c"]);
        // Whitespace-only query behaves like no filter.
        let out = apply_filters(list(), "   ", StatusFilter::All, "all", SortMode::Newest);
        assert_eq!(out.len(), 4);
        let out = apply_filters(list(), "nope", StatusFilter::All, "all", SortMode::Newest);
        assert!(out.is_empty());
    }

    #[test]
    fn status_and_agent_filters_apply() {
        let locked = apply_filters(list(), "", StatusFilter::Locked, "all", SortMode::Newest);
        assert_eq!(ids(&locked), ["b"]);
        let free = apply_filters(list(), "", StatusFilter::Free, "all", SortMode::Newest);
        assert_eq!(ids(&free), ["a", "c", "d"]);
        let agent = apply_filters(list(), "", StatusFilter::All, "cline", SortMode::Newest);
        assert_eq!(ids(&agent), ["b"]);
        let missing = apply_filters(list(), "", StatusFilter::All, "ghost", SortMode::Newest);
        assert!(missing.is_empty());
    }

    #[test]
    fn sorts_by_newest_oldest_and_title() {
        let newest = apply_filters(list(), "", StatusFilter::All, "all", SortMode::Newest);
        assert_eq!(ids(&newest), ["b", "a", "c", "d"]);
        let oldest = apply_filters(list(), "", StatusFilter::All, "all", SortMode::Oldest);
        assert_eq!(ids(&oldest), ["d", "c", "a", "b"]);
        // Title sort is case-insensitive; the empty title ranks first.
        let title = apply_filters(list(), "", StatusFilter::All, "all", SortMode::Title);
        assert_eq!(ids(&title), ["d", "c", "b", "a"]);
    }

    #[test]
    fn groups_order_by_recency_with_uncategorized_last() {
        let groups = group_by_cwd(list());
        assert_eq!(groups.len(), 3);
        // `/home/u/app` has the newest row (10-09), `/var/www/site`
        // next (10-07), and the empty cwd trails.
        assert_eq!(groups[0].0, "/home/u/app");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "/var/www/site");
        assert_eq!(groups[2].0, "");
        assert_eq!(groups[2].1.len(), 1);
        // Empty input → no groups.
        assert!(group_by_cwd(Vec::new()).is_empty());
    }

    #[test]
    fn cwd_label_uses_last_path_segment() {
        assert_eq!(cwd_label("/home/u/app"), "app");
        assert_eq!(cwd_label("/home/u/app/"), "app");
        assert_eq!(cwd_label("app"), "app");
        assert_eq!(cwd_label(""), "No project");
        assert_eq!(cwd_label("/"), "No project");
        assert_eq!(cwd_label("C:\\src\\proj"), "proj");
    }

    #[test]
    fn select_parse_round_trips() {
        assert_eq!(SortMode::parse("oldest"), SortMode::Oldest);
        assert_eq!(SortMode::parse("bogus"), SortMode::Newest);
        assert_eq!(SortMode::parse(SortMode::Title.as_str()), SortMode::Title);
        assert_eq!(StatusFilter::parse("locked"), StatusFilter::Locked);
        assert_eq!(StatusFilter::parse("bogus"), StatusFilter::All);
        assert_eq!(
            StatusFilter::parse(StatusFilter::Free.as_str()),
            StatusFilter::Free
        );
    }
}
