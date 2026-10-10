//! `/nodes` — the node's identity (`GET /api/node`), per-node health
//! from the hub registry / sync projection, and the hub's queued
//! writes (the outbox a down node's prompts/cancels wait in).

use leptos::prelude::*;
use leptos_meta::Title;

use crate::api::{redeem_pair_code, rename_node};
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ErrorBanner, Input, PageDescription, PageHead, PageTitle, SELECT_CLASS,
    Skeleton,
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
                        // Handle copies for the retry buttons — the
                        // awaits below shadow the resource names.
                        let (info_r, status_r, pending_r) = (info, status, pending);
                        let (info, status, pending) =
                            (info.await, status.await, pending.await);
                        let info_view = match info {
                            Err(e) => {
                                Some(
                                    view! {
                                        <ErrorBanner
                                            message=e.to_string()
                                            on_retry=Box::new(move || info_r.refetch())
                                        />
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
                                                    <dt class="text-muted-foreground">"name"</dt>
                                                    <dd>
                                                        <NicknameEdit
                                                            name=n.name.clone()
                                                        />
                                                    </dd>
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
                                    <ErrorBanner
                                        message=e.to_string()
                                        on_retry=Box::new(move || status_r.refetch())
                                    />
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
                                    <ErrorBanner
                                        message=e.to_string()
                                        on_retry=Box::new(move || pending_r.refetch())
                                    />
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
            <PairCard/>
        </section>
    }
}

/// "Pair a device" — redeem a one-time code (`sepia pair` writes it
/// to `$SEPIA_HOME/pair-code` on the node) for a long-lived node
/// credential. `POST /api/pair` is deliberately unauthenticated
/// node-side — the code itself is the credential.
#[component]
fn PairCard() -> impl IntoView {
    let client = crate::api::query_client();
    let status = client.resource(crate::api::node_status_scope, || ());
    let code = RwSignal::new(String::new());
    let node = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let toast = use_toast();
    // `Some(Ok(token))` once redeemed, `Some(Err(msg))` on failure.
    let result: RwSignal<Option<Result<String, String>>> = RwSignal::new(None);

    let submit = move || {
        let c = code.get_untracked();
        if c.trim().is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        result.set(None);
        let node_arg = {
            let n = node.get_untracked();
            (!n.is_empty()).then_some(n)
        };
        leptos::task::spawn_local(async move {
            let res = redeem_pair_code(c, node_arg)
                .await
                .map_err(|e| e.to_string());
            busy.set(false);
            match &res {
                Ok(_) => {
                    toast.success("Device paired — store the credential somewhere safe.");
                    code.set(String::new());
                }
                Err(e) => toast.error(e.clone()),
            }
            result.set(Some(res));
        });
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Pair a device"</CardTitle>
                <CardDescription>
                    "Redeem the one-time code from `sepia pair` for a node credential. The token is shown once — store it somewhere safe."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <form
                    data-name="PairCard"
                    class="flex flex-col gap-3"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        submit();
                    }
                >
                    <div class="flex flex-wrap items-end gap-2">
                        <label class="flex min-w-48 flex-1 flex-col gap-1.5 text-sm">
                            <span class="text-muted-foreground">"Pairing code"</span>
                            <Input
                                {..}
                                attr:r#type="text"
                                attr:name="code"
                                attr:placeholder="XXXX-XXXX"
                                attr:autocomplete="off"
                                prop:value=move || code.get()
                                on:input=move |ev| code.set(event_target_value(&ev))
                            />
                        </label>
                        // Target node — only meaningful on multi-node
                        // hubs; hidden as a lone "auto" option else.
                        <Suspense fallback=move || {
                            view! { <span class="hidden"></span> }
                        }>
                            {move || {
                                Suspend::new(async move {
                                    let rows = status.await.unwrap_or_default();
                                    if rows.len() > 1 {
                                view! {
                                    <label class="flex flex-col gap-1.5 text-sm">
                                        <span class="text-muted-foreground">"Node"</span>
                                        <select
                                            class=SELECT_CLASS
                                            prop:value=move || node.get()
                                            on:change=move |ev| {
                                                node.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="">"primary"</option>
                                            {rows
                                                .into_iter()
                                                .map(|r| {
                                                    let v = r.id.clone();
                                                    view! { <option value=v.clone()>{v.clone()}</option> }
                                                })
                                                .collect::<Vec<_>>()}
                                        </select>
                                    </label>
                                        }
                                            .into_any()
                                    } else {
                                        view! { <span class="hidden"></span> }.into_any()
                                    }
                                })
                            }}
                        </Suspense>
                        <Button
                            button_type="submit"
                            size=ButtonSize::Default
                            disabled=move || busy.get()
                        >
                            {move || if busy.get() { "Pairing…" } else { "Pair" }}
                        </Button>
                    </div>
                </form>
                {move || {
                    result.get().map(|res| {
                        match res {
                            Ok(token) => {
                                view! {
                                    <div data-name="PairSuccess" class="mt-3">
                                        <p class="text-sm text-success">
                                            "Paired — node credential (shown once):"
                                        </p>
                                        <code class="mt-1 block break-all rounded-md border bg-muted px-3 py-2 font-mono text-xs">
                                            {token}
                                        </code>
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(e) => {
                                view! {
                                    <p
                                        data-name="PairError"
                                        role="alert"
                                        class="mt-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                                    >
                                        {e}
                                    </p>
                                }
                                    .into_any()
                            }
                        }
                    })
                }}
            </CardContent>
        </Card>
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

/// Inline nickname editor — the pencil swaps the name for an input;
/// save hits `PATCH /api/node` (`rename_node`) and toasts the result.
/// `node` picks the target on multi-node hubs (`None` = primary).
/// When the node answers, the descriptor refresh invalidates the
/// shared `node_info`/`node_status` queries.
#[component]
pub(crate) fn NicknameEdit(
    #[prop(into)] name: String,
    #[prop(into, optional)] node: Option<String>,
    #[prop(into, optional)] class: String,
) -> impl IntoView {
    let client = crate::api::query_client();
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let toast = use_toast();
    let display = if name.is_empty() {
        "—".to_string()
    } else {
        name
    };

    view! {
        <span
            data-name="NicknameEdit"
            class=tw_merge::tw_merge!("inline-flex items-center gap-1.5", class)
        >
            {move || {
                if editing.get() {
                    // Fresh clones per render — the submit handler
                    // moves them, and this closure is `FnMut`.
                    let node = node.clone();
                    view! {
                        <form
                            class="inline-flex items-center gap-1.5"
                            on:submit=move |ev| {
                                ev.prevent_default();
                                let name_arg = draft.get();
                                if name_arg.trim().is_empty() || busy.get() {
                                    return;
                                }
                                busy.set(true);
                                let node_arg = node.clone();
                                leptos::task::spawn_local(async move {
                                    if toast
                                        .outcome(
                                            rename_node(name_arg, node_arg).await,
                                            "Node renamed.",
                                        )
                                        .is_some()
                                    {
                                        editing.set(false);
                                        client.invalidate_query(
                                            crate::api::node_info_scope,
                                            (),
                                        );
                                        client.invalidate_query(
                                            crate::api::node_status_scope,
                                            (),
                                        );
                                    }
                                    busy.set(false);
                                });
                            }
                        >
                            <Input
                                class="h-7 w-44 px-2 text-xs"
                                attr:r#type="text"
                                attr:maxlength=100
                                attr:aria-label="Node nickname"
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        editing.set(false);
                                    }
                                }
                            />
                            <Button
                                button_type="submit"
                                size=ButtonSize::Sm
                                disabled=move || busy.get() || draft.read().trim().is_empty()
                            >
                                {move || if busy.get() { "Saving…" } else { "Save" }}
                            </Button>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on_click=Box::new(move || editing.set(false))
                            >
                                "Cancel"
                            </Button>
                        </form>
                    }
                        .into_any()
                } else {
                    let shown = display.clone();
                    let seed = display.clone();
                    view! {
                        <span>{shown}</span>
                        <button
                            type="button"
                            aria-label="Rename node"
                            title="Rename node"
                            class="text-muted-foreground hover:text-foreground"
                            on:click=move |_| {
                                draft.set(seed.clone());
                                editing.set(true);
                            }
                        >
                            "✎"
                        </button>
                    }
                        .into_any()
                }
            }}
        </span>
    }
}

#[component]
pub(crate) fn NodeRow(row: NodeStatusDto) -> impl IntoView {
    let variant = status_variant(&row.status);
    let nickname = if row.label.is_empty() {
        row.id.clone()
    } else {
        row.label.clone()
    };
    view! {
        <li class="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-3 first:pt-0 last:pb-0">
            <Badge variant=variant>{row.status.clone()}</Badge>
            <span class="text-sm font-medium">
                <NicknameEdit name=nickname node=row.id.clone()/>
                {if row.label.is_empty() || row.label == row.id {
                    None
                } else {
                    let id = row.id.clone();
                    Some(
                        view! {
                            <code class="ml-1.5 font-mono text-xs text-muted-foreground">
                                {format!("({id})")}
                            </code>
                        },
                    )
                }}
            </span>
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
