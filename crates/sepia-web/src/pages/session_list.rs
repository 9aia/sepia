//! `/` — the merged session list. SSR renders the first page; on the
//! client the list refreshes when `/api/events` says a session row
//! changed, and on a 30s ticker (relative timestamps).

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

#[cfg(feature = "hydrate")]
use crate::api::list_agents;
use crate::api::{create_session, list_sessions, pending_writes};
use crate::app::Now;
use crate::components::{
    Badge, BadgeVariant, Button, Input, PageDescription, PageHead, PageTitle, SELECT_CLASS,
    Skeleton,
};
use crate::dto::AgentDto;
use crate::dto::SessionSummaryDto;
use crate::time::relative;

#[component]
pub fn SessionListPage() -> impl IntoView {
    let sessions = Resource::new(|| (), |()| list_sessions());
    let pending = Resource::new(|| (), |()| pending_writes());

    // Refresh when the node feed reports a session/meta/project change.
    // Closed with the page (EventSource has no Drop impl).
    #[cfg(feature = "hydrate")]
    {
        // `EventStream` isn't `Send` (wasm closures); `new_local` keeps
        // it in the component's arena and its `Drop` closes the feed.
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || {
            sessions.refetch();
            pending.refetch();
        }));
        // Outbox drain/enqueue emits no feed event — poll slowly.
        crate::app::every_ms(30_000, move || pending.refetch());
    }

    view! {
        <Title text="sessions — sepia"/>
        <section>
            <PageHead>
                <div>
                    <PageTitle>"Sessions"</PageTitle>
                    <PageDescription>
                        "Agent sessions across all of your nodes."
                    </PageDescription>
                </div>
            </PageHead>
            <NewSessionForm on_created=move || sessions.refetch()/>
            <Suspense fallback=move || {
                view! {
                    <div class="flex flex-col gap-2">
                        <Skeleton class="h-16 w-full"/>
                        <Skeleton class="h-16 w-full"/>
                        <Skeleton class="h-16 w-full"/>
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
                                    <p class="whitespace-pre-wrap font-mono text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">
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
                                    <ul class="flex flex-col gap-2">
                                        {list
                                            .into_iter()
                                            .map(|s| {
                                                let (queued, failed) = writes
                                                    .get(&s.id)
                                                    .copied()
                                                    .unwrap_or_default();
                                                view! {
                                                    <SessionRow
                                                        session=s
                                                        queued=queued
                                                        failed=failed
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
        </section>
    }
}

#[component]
fn SessionRow(session: SessionSummaryDto, queued: usize, failed: usize) -> impl IntoView {
    let title = if session.title.trim().is_empty() {
        "Untitled session".to_string()
    } else {
        session.title.clone()
    };
    let href = match &session.agent {
        agent if agent.is_empty() => format!("/sessions/{}", session.id),
        agent => format!("/sessions/{}?agent={agent}", session.id),
    };
    let iso = session.updated_at.clone();
    view! {
        <li>
            <A
                href=href
                attr:class="block rounded-lg border bg-card p-3 transition-colors hover:bg-accent/50"
            >
                <span class="flex items-center justify-between gap-2">
                    <span class="min-w-0 truncate font-medium">{title}</span>
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
                <span class="mt-1.5 flex items-center gap-2 text-xs text-muted-foreground">
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
                        format!("/sessions/{}", res.id)
                    } else {
                        format!("/sessions/{}?agent={}", res.id, res.agent_id)
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
            class="mb-6 flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center"
            on:submit=move |ev| {
                ev.prevent_default();
                create();
            }
        >
            <Input
                attr:r#type="text"
                attr:placeholder="Working directory (required)…"
                class="sm:flex-[2]"
                prop:value=move || cwd.get()
                on:input=move |ev| cwd.set(event_target_value(&ev))
            />
            <Input
                attr:r#type="text"
                attr:placeholder="Title (optional)…"
                attr:maxlength="200"
                class="sm:flex-1"
                prop:value=move || title.get()
                on:input=move |ev| title.set(event_target_value(&ev))
            />
            <select
                class=format!("{SELECT_CLASS} sm:w-44 sm:shrink-0")
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
            <Input
                attr:r#type="text"
                attr:placeholder="Model (optional)…"
                attr:maxlength="100"
                class="sm:flex-1"
                prop:value=move || model.get()
                on:input=move |ev| model.set(event_target_value(&ev))
            />
            <Button
                button_type="submit"
                disabled=Signal::derive(move || {
                    creating.get() || cwd.read().trim().is_empty()
                })
            >
                {move || if creating.get() { "Creating…" } else { "New session" }}
            </Button>
            {move || {
                form_error
                    .get()
                    .map(|e| view! {
                        <p class="basis-full text-sm text-destructive">{e}</p>
                    })
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
