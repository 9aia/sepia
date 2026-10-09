//! `/nodes` — the node's identity (`GET /api/node`), per-node health
//! from the hub registry / sync projection, and the hub's queued
//! writes (the outbox a down node's prompts/cancels wait in).

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::{
    Badge, BadgeVariant, Card, CardContent, CardHeader, CardTitle, PageDescription, PageHead,
    PageTitle, Skeleton,
};
use crate::dto::{NodeStatusDto, PendingWriteDto};
use crate::pages::RelativeTime;

#[component]
pub fn NodesPage() -> impl IntoView {
    let client = crate::api::query_client();
    let info = client.resource(crate::api::node_info_scope, || ());
    let status = client.resource(crate::api::node_status_scope, || ());
    let pending = client.resource(crate::api::pending_scope, || ());

    #[cfg(feature = "hydrate")]
    {
        // Outbox drain/enqueue emits no feed event — poll slowly.
        crate::app::every_ms(30_000, move || {
            status.refetch();
            pending.refetch();
        });
    }

    view! {
        <Title text="nodes — sepia"/>
        <section class="mx-auto w-full max-w-5xl space-y-6 p-4 md:p-6">
            <PageHead class="mb-0">
                <div>
                    <PageTitle>"Nodes"</PageTitle>
                    <PageDescription>
                        "Node identity, hub health, and the queued outbox."
                    </PageDescription>
                </div>
            </PageHead>
            <Suspense fallback=move || {
                view! {
                    <div class="grid gap-4">
                        <Skeleton class="h-44 w-full"/>
                        <Skeleton class="h-32 w-full"/>
                        <Skeleton class="h-24 w-full"/>
                    </div>
                }
            }>
                {move || {
                    Suspend::new(async move {
                        let (info, status, pending) =
                            (info.await, status.await, pending.await);
                        let info_view = match info {
                            Err(e) => {
                                Some(
                                    view! {
                                        <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                            {e.to_string()}
                                        </p>
                                    }
                                        .into_any(),
                                )
                            }
                            Ok(n) => {
                                let title = if n.name.is_empty() {
                                    n.id.clone()
                                } else {
                                    n.name.clone()
                                };
                                let version =
                                    format!("{} (protocol {})", n.version, n.protocol);
                                Some(
                                    view! {
                                        <Card>
                                            <CardHeader>
                                                <CardTitle>{title}</CardTitle>
                                            </CardHeader>
                                            <CardContent>
                                                <dl class="grid grid-cols-[6rem_1fr] gap-x-4 gap-y-2.5 text-sm">
                                                    <dt class="text-muted-foreground">"id"</dt>
                                                    <dd class="font-mono text-xs">{n.id.clone()}</dd>
                                                    <dt class="text-muted-foreground">"version"</dt>
                                                    <dd>{version}</dd>
                                                    <dt class="text-muted-foreground">"agents"</dt>
                                                    <dd>
                                                        <div class="flex flex-wrap gap-1.5">
                                                            {if n.agents.is_empty() {
                                                                view! {
                                                                    <span class="text-xs text-muted-foreground">"none"</span>
                                                                }
                                                                    .into_any()
                                                            } else {
                                                                n.agents
                                                                    .iter()
                                                                    .map(|a| {
                                                                        let a = a.clone();
                                                                        view! {
                                                                            <Badge variant=BadgeVariant::Muted>{a}</Badge>
                                                                        }
                                                                    })
                                                                    .collect::<Vec<_>>()
                                                                    .into_any()
                                                            }}
                                                        </div>
                                                    </dd>
                                                    <dt class="text-muted-foreground">"capabilities"</dt>
                                                    <dd>
                                                        <div class="flex flex-wrap gap-1.5">
                                                            {n.capabilities
                                                                .iter()
                                                                .map(|c| {
                                                                    let c = c.clone();
                                                                    view! {
                                                                        <Badge variant=BadgeVariant::Muted>{c}</Badge>
                                                                    }
                                                                })
                                                                .collect::<Vec<_>>()}
                                                        </div>
                                                    </dd>
                                                </dl>
                                            </CardContent>
                                        </Card>
                                    }
                                        .into_any(),
                                )
                            }
                        };
                        let status_view = match status {
                            Err(e) => {
                                view! {
                                    <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(rows) if rows.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No nodes registered."
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(rows) => {
                                view! {
                                    <Card>
                                        <CardHeader>
                                            <CardTitle>"Health"</CardTitle>
                                        </CardHeader>
                                        <CardContent>
                                            <ul class="divide-y divide-border">
                                                {rows
                                                    .into_iter()
                                                    .map(|r| view! { <NodeRow row=r/> })
                                                    .collect::<Vec<_>>()}
                                            </ul>
                                        </CardContent>
                                    </Card>
                                }
                                    .into_any()
                            }
                        };
                        let pending_view = match pending {
                            Err(e) => {
                                view! {
                                    <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                                        {e.to_string()}
                                    </p>
                                }
                                    .into_any()
                            }
                            Ok(rows) => {
                                view! {
                                    <Card>
                                        <CardHeader>
                                            <CardTitle>"Queued writes"</CardTitle>
                                        </CardHeader>
                                        <CardContent>
                                            {if rows.is_empty() {
                                                view! {
                                                    <p class="text-sm text-muted-foreground">
                                                        "No queued writes."
                                                    </p>
                                                }
                                                    .into_any()
                                            } else {
                                                view! {
                                                    <ul class="divide-y divide-border">
                                                        {rows
                                                            .into_iter()
                                                            .map(|w| view! { <WriteRow write=w/> })
                                                            .collect::<Vec<_>>()}
                                                    </ul>
                                                }
                                                    .into_any()
                                            }}
                                        </CardContent>
                                    </Card>
                                }
                                    .into_any()
                            }
                        };
                        view! {
                            {info_view}
                            {status_view}
                            {pending_view}
                        }
                    })
                }}
            </Suspense>
        </section>
    }
}

/// Maps the node's reported status string onto a badge variant.
pub(crate) fn status_variant(status: &str) -> BadgeVariant {
    match status {
        "ok" | "up" | "connected" => BadgeVariant::Success,
        "idle" | "queued" => BadgeVariant::Info,
        "locked" | "busy" => BadgeVariant::Warning,
        "down" | "failed" | "error" => BadgeVariant::Destructive,
        _ => BadgeVariant::Muted,
    }
}

#[component]
pub(crate) fn NodeRow(row: NodeStatusDto) -> impl IntoView {
    let variant = status_variant(&row.status);
    let label = if row.label.is_empty() || row.label == row.id {
        row.id.clone()
    } else {
        format!("{} ({})", row.label, row.id)
    };
    view! {
        <li class="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-3 first:pt-0 last:pb-0">
            <Badge variant=variant>{row.status.clone()}</Badge>
            <span class="text-sm font-medium">{label}</span>
            <code class="font-mono text-xs text-muted-foreground">{row.url.clone()}</code>
            <span class="ml-auto text-xs text-muted-foreground">
                {row
                    .last_seen_at
                    .clone()
                    .map(|t| view! { <RelativeTime iso=t/> })}
            </span>
        </li>
    }
}

/// One outbox row — `queued` writes replay when the node returns;
/// `failed` (dead-lettered) ones carry the error they died on.
#[component]
#[allow(clippy::needless_pass_by_value)] // component props are owned
pub(crate) fn WriteRow(write: PendingWriteDto) -> impl IntoView {
    let failed = write.status == "failed";
    let variant = if failed {
        BadgeVariant::Destructive
    } else {
        BadgeVariant::Info
    };
    let status = write.status.clone();
    let attempts = write.attempts;
    let detail = format!("{} → {}", write.node_id, write.session_id);
    view! {
        <li class="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-3 first:pt-0 last:pb-0">
            <Badge variant=variant>{status}</Badge>
            <code class="font-mono text-xs font-medium">{write.op.clone()}</code>
            <span class="font-mono text-xs text-muted-foreground">{detail}</span>
            <span class="ml-auto flex items-center gap-x-3">
                {write
                    .last_error
                    .clone()
                    .map(|e| view! { <span class="text-xs text-destructive">{e}</span> })}
                <span class="text-xs text-muted-foreground">
                    {format!("{attempts} attempt(s)")}
                </span>
                <RelativeTime iso=write.enqueued_at.clone()/>
            </span>
        </li>
    }
}
