//! `/` — the merged session list. SSR renders the first page; on the
//! client the list refreshes when `/api/events` says a session row
//! changed, and on a 30s ticker (relative timestamps).

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

use crate::api::list_sessions;
use crate::app::Now;
use crate::dto::SessionSummaryDto;
use crate::time::relative;

#[component]
pub fn SessionListPage() -> impl IntoView {
    let sessions = Resource::new(|| (), |()| list_sessions());

    // Refresh when the node feed reports a session/meta/project change.
    // Closed with the page (EventSource has no Drop impl).
    #[cfg(feature = "hydrate")]
    {
        // `EventStream` isn't `Send` (wasm closures); `new_local` keeps
        // it in the component's arena and its `Drop` closes the feed.
        let _feed = StoredValue::new_local(crate::sse::node_feed(move || sessions.refetch()));
    }

    view! {
        <Title text="sessions — sepia"/>
        <section class="page">
            <header class="page-head">
                <h1>"Sessions"</h1>
            </header>
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading sessions…"</p> }
            }>
                {move || {
                    Suspend::new(async move {
                        match sessions.await {
                            Err(e) => {
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! { <p class="empty">"No sessions yet."</p> }
                                    .into_any()
                            }
                            Ok(list) => {
                                view! {
                                    <ul class="session-list">
                                        {list
                                            .into_iter()
                                            .map(|s| view! { <SessionRow session=s/> })
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
fn SessionRow(session: SessionSummaryDto) -> impl IntoView {
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
