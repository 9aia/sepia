//! `/agents` — the agent runtime inventory. On a multi-node hub the
//! list is merged and each row carries its owning `node` badge.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::list_agents;
use crate::dto::{AgentCapabilitiesDto, AgentDto};

#[component]
pub fn AgentsPage() -> impl IntoView {
    let agents = Resource::new(|| (), |()| list_agents());

    view! {
        <Title text="agents — sepia"/>
        <section class="page">
            <header class="page-head">
                <h1>"Agents"</h1>
            </header>
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading agents…"</p> }
            }>
                {move || {
                    Suspend::new(async move {
                        match agents.await {
                            Err(e) => {
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! { <p class="empty">"No agents configured."</p> }.into_any()
                            }
                            Ok(list) => {
                                view! {
                                    <ul class="card-list">
                                        {list
                                            .into_iter()
                                            .map(|a| view! { <AgentCard agent=a/> })
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
fn AgentCard(agent: AgentDto) -> impl IntoView {
    let label = if agent.label.trim().is_empty() {
        agent.id.clone()
    } else {
        agent.label.clone()
    };
    let chips = capability_chips(agent.capabilities.as_ref());
    view! {
        <li class="card">
            <div class="card-head">
                <span class="card-title">{label}</span>
                <span class="badges">
                    {agent.node.clone().map(|n| view! { <span class="badge node">{n}</span> })}
                </span>
            </div>
            <p class="card-meta">
                <code>{agent.id.clone()}</code>
            </p>
            <div class="chips">
                {if chips.is_empty() {
                    view! { <span class="empty">"capabilities not probed"</span> }.into_any()
                } else {
                    chips
                        .into_iter()
                        .map(|c| view! { <span class="badge cap">{c}</span> })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </div>
        </li>
    }
}

/// The display-facing capability chips — a flat name list derived from
/// the wire's `capabilities` object.
fn capability_chips(caps: Option<&AgentCapabilitiesDto>) -> Vec<&'static str> {
    let Some(c) = caps else {
        return Vec::new();
    };
    let mut chips = Vec::new();
    if c.load_session {
        chips.push("loadSession");
    }
    if c.session_list || c.session_capabilities.list {
        chips.push("list");
    }
    if c.session_capabilities.delete {
        chips.push("delete");
    }
    if c.prompt_capabilities.image {
        chips.push("image");
    }
    if c.prompt_capabilities.audio {
        chips.push("audio");
    }
    if c.prompt_capabilities.embedded_context {
        chips.push("embeddedContext");
    }
    chips
}
