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

    // Refresh when the node feed reports a session/meta/project change.
    #[cfg(feature = "hydrate")]
    {
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            sessions.refetch();
            pending.refetch();
        }));
        crate::app::every_ms(30_000, move || pending.refetch());
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
                if selected().is_some() {
                    format!("hidden {base}")
                } else {
                    format!("flex {base} flex-1")
                }
            }>
                <div class="border-b p-3">
                    <NewSessionForm on_created=move || sessions.refetch()/>
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
                                        view! {
                                            <p class="whitespace-pre-wrap p-2 font-mono text-sm text-destructive">
                                                {e.to_string()}
                                            </p>
                                        }
                                            .into_any()
                                    }
                                    Ok(list) if list.is_empty() => {
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
                                        view! {
                                            <ul class="flex flex-col gap-0.5">
                                                {list
                                                    .into_iter()
                                                    .map(|s| {
                                                        let (queued, failed) = writes
                                                            .get(&s.id)
                                                            .copied()
                                                            .unwrap_or_default();
                                                        let row_id = s.id.clone();
                                                        let is_selected = Signal::derive(move || {
                                                            selected_id.get().as_deref()
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
                                                    .collect::<Vec<_>>()}
                                            </ul>
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
/// id (or the empty "default") goes to the primary.
#[component]
fn NewSessionForm(on_created: impl Fn() + 'static + Send + Sync + Copy) -> impl IntoView {
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
            <Input
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
