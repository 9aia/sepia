//! `/` — the merged session list. SSR renders the first page; on the
//! client the list refreshes when `/api/events` says a session row
//! changed, and on a 30s ticker (relative timestamps).

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

use crate::api::{create_session, list_agents, list_sessions, pending_writes};
use crate::app::Now;
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
        <section class="page">
            <header class="page-head">
                <h1>"Sessions"</h1>
            </header>
            <NewSessionForm on_created=move || sessions.refetch()/>
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading sessions…"</p> }
            }>
                {move || {
                    Suspend::new(async move {
                        // A broken outbox read never sinks the list.
                        let pending_result = pending.await.unwrap_or_default();
                        match sessions.await {
                            Err(e) => {
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! { <p class="empty">"No sessions yet."</p> }
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
                                    <ul class="session-list">
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
        <li class="session">
            <A href=href attr:class="session-link">
                <span class="session-top">
                    <span class="session-title">{title}</span>
                    <span class="badges">
                        {session.node.clone().map(|n| view! { <span class="badge node">{n}</span> })}
                        {session.busy.then(|| view! { <span class="badge busy">"busy"</span> })}
                        {session.locked.then(|| view! { <span class="badge locked">"locked"</span> })}
                        {(queued > 0).then(|| {
                            let label = if queued > 1 {
                                format!("queued ×{queued}")
                            } else {
                                "queued".to_string()
                            };
                            view! { <span class="badge queued">{label}</span> }
                        })}
                        {(failed > 0).then(|| {
                            let label = if failed > 1 {
                                format!("failed ×{failed}")
                            } else {
                                "failed".to_string()
                            };
                            view! { <span class="badge failed">{label}</span> }
                        })}
                        {session.pinned.then(|| view! { <span class="badge pinned">"pinned"</span> })}
                    </span>
                </span>
                <span class="session-meta">
                    <span class="agent">{session.agent.clone()}</span>
                    <span class="cwd">{session.cwd.clone()}</span>
                    <RelativeTime iso=iso/>
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
    let agents = Resource::new(|| (), |()| list_agents());
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
            class="form-row new-session"
            on:submit=move |ev| {
                ev.prevent_default();
                create();
            }
        >
            <input
                class="field"
                type="text"
                placeholder="Working directory (required)…"
                prop:value=move || cwd.get()
                on:input=move |ev| cwd.set(event_target_value(&ev))
            />
            <input
                class="field"
                type="text"
                placeholder="Title (optional)…"
                maxlength=200
                prop:value=move || title.get()
                on:input=move |ev| title.set(event_target_value(&ev))
            />
            <select
                class="field agent-select"
                prop:value=move || agent_sel.get()
                on:change=move |ev| agent_sel.set(event_target_value(&ev))
            >
                <option value="">"default agent"</option>
                {move || {
                    agents
                        .get()
                        .and_then(Result::ok)
                        .map(|list| {
                            list
                                .into_iter()
                                .map(|a| {
                                    let value = match &a.node {
                                        Some(n) if !n.is_empty() => {
                                            format!("{n}|{}", a.id)
                                        }
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
                        })
                }}
            </select>
            <input
                class="field"
                type="text"
                placeholder="Model (optional)…"
                maxlength=100
                prop:value=move || model.get()
                on:input=move |ev| model.set(event_target_value(&ev))
            />
            <button
                class="send"
                type="submit"
                disabled=move || creating.get() || cwd.read().trim().is_empty()
            >
                {move || if creating.get() { "Creating…" } else { "New session" }}
            </button>
            {move || form_error.get().map(|e| view! { <p class="error">{e}</p> })}
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
        <time class="time" datetime=iso.clone() title=iso>
            {label}
        </time>
    }
}
