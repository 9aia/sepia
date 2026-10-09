//! `/agents` — the agent runtime inventory. On a multi-node hub the
//! list is merged and each row carries its owning `node` badge.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::list_agents;
use crate::components::{
    Badge, BadgeVariant, Card, CardContent, CardHeader, CardTitle, PageDescription, PageHead,
    PageTitle, Skeleton,
};
use crate::dto::{AgentCapabilitiesDto, AgentDto};

#[component]
pub fn AgentsPage() -> impl IntoView {
    let agents = Resource::new(|| (), |()| list_agents());

    view! {
        <Title text="agents — sepia"/>
        <section class="mx-auto w-full max-w-5xl space-y-6 p-4 md:p-6">
            <PageHead class="mb-0">
                <div>
                    <PageTitle>"Agents"</PageTitle>
                    <PageDescription>
                        "Agent runtimes advertised by the connected nodes."
                    </PageDescription>
                </div>
            </PageHead>
            <Suspense fallback=move || {
                view! {
                    <div class="grid gap-3">
                        <Skeleton class="h-28 w-full"/>
                        <Skeleton class="h-28 w-full"/>
                    </div>
                }
            }>
                {move || {
                    Suspend::new(async move {
                        match agents.await {
                            Err(e) => {
                                view! {
                                    <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(list) if list.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No agents configured."
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(list) => {
                                view! {
                                    <ul class="grid gap-3">
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
        <li>
            <Card>
                <CardHeader class="flex-row items-center justify-between gap-3">
                    <CardTitle>{label}</CardTitle>
                    {agent
                        .node
                        .clone()
                        .map(|n| view! { <Badge variant=BadgeVariant::Info>{n}</Badge> })}
                </CardHeader>
                <CardContent class="flex flex-col gap-3">
                    <p class="font-mono text-xs text-muted-foreground">{agent.id.clone()}</p>
                    <div class="flex flex-wrap gap-1.5">
                        {if chips.is_empty() {
                            view! {
                                <span class="text-xs text-muted-foreground">
                                    "capabilities not probed"
                                </span>
                            }
                                .into_any()
                        } else {
                            chips
                                .into_iter()
                                .map(|c| view! { <Badge variant=BadgeVariant::Muted>{c}</Badge> })
                                .collect::<Vec<_>>()
                                .into_any()
                        }}
                    </div>
                </CardContent>
            </Card>
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
