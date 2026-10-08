//! `/nodes` — the node's identity (`GET /api/node`) plus per-node
//! health from the hub registry / sync projection.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::{node_info, node_status};
use crate::dto::NodeStatusDto;
use crate::pages::RelativeTime;

#[component]
pub fn NodesPage() -> impl IntoView {
    let info = Resource::new(|| (), |()| node_info());
    let status = Resource::new(|| (), |()| node_status());

    view! {
        <Title text="nodes — sepia"/>
        <section class="page">
            <header class="page-head">
                <h1>"Nodes"</h1>
            </header>
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading nodes…"</p> }
            }>
                {move || {
                    Suspend::new(async move {
                        let (info, status) = (info.await, status.await);
                        let info_view = match info {
                            Err(e) => {
                                Some(view! { <p class="error">{e.to_string()}</p> }.into_any())
                            }
                            Ok(n) => {
                                Some(
                                    view! {
                                        <section class="settings-section">
                                            <h2 class="section-head">
                                                {if n.name.is_empty() {
                                                    n.id.clone()
                                                } else {
                                                    n.name.clone()
                                                }}
                                            </h2>
                                            <dl class="facts">
                                                <dt>"id"</dt>
                                                <dd>
                                                    <code>{n.id.clone()}</code>
                                                </dd>
                                                <dt>"version"</dt>
                                                <dd>{format!("{} (protocol {})", n.version, n.protocol)}</dd>
                                                <dt>"agents"</dt>
                                                <dd>
                                                    <div class="chips">
                                                        {if n.agents.is_empty() {
                                                            view! { <span class="empty">"none"</span> }
                                                                .into_any()
                                                        } else {
                                                            n.agents
                                                                .iter()
                                                                .map(|a| view! { <span class="badge cap">{a.clone()}</span> })
                                                                .collect::<Vec<_>>()
                                                                .into_any()
                                                        }}
                                                    </div>
                                                </dd>
                                                <dt>"capabilities"</dt>
                                                <dd>
                                                    <div class="chips">
                                                        {n.capabilities
                                                            .iter()
                                                            .map(|c| {
                                                                view! { <span class="badge cap">{c.clone()}</span> }
                                                            })
                                                            .collect::<Vec<_>>()}
                                                    </div>
                                                </dd>
                                            </dl>
                                        </section>
                                    }
                                        .into_any(),
                                )
                            }
                        };
                        let status_view = match status {
                            Err(e) => view! { <p class="error">{e.to_string()}</p> }.into_any(),
                            Ok(rows) if rows.is_empty() => {
                                view! { <p class="empty">"No nodes registered."</p> }.into_any()
                            }
                            Ok(rows) => {
                                view! {
                                    <section class="settings-section">
                                        <h2 class="section-head">"Health"</h2>
                                        <ul class="node-list">
                                            {rows
                                                .into_iter()
                                                .map(|r| view! { <NodeRow row=r/> })
                                                .collect::<Vec<_>>()}
                                        </ul>
                                    </section>
                                }
                                    .into_any()
                            }
                        };
                        view! {
                            {info_view}
                            {status_view}
                        }
                    })
                }}
            </Suspense>
        </section>
    }
}

#[component]
fn NodeRow(row: NodeStatusDto) -> impl IntoView {
    let class = format!("badge status {}", row.status);
    let label = if row.label.is_empty() || row.label == row.id {
        row.id.clone()
    } else {
        format!("{} ({})", row.label, row.id)
    };
    view! {
        <li class="node-row">
            <span class=class>{row.status.clone()}</span>
            <span class="node-label">{label}</span>
            <code class="node-url">{row.url.clone()}</code>
            {row
                .last_seen_at
                .clone()
                .map(|t| view! { <RelativeTime iso=t/> })}
        </li>
    }
}
