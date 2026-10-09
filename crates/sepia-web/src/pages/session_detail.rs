//! `/sessions/:id` — history (paged via `?before`), live SSE stream
//! folded into a transcript, and the prompt box.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};

use crate::api::{
    answer_permission, attach_session, cancel_run, delete_session, detach_session, get_session,
    list_checkpoints, pending_writes, rename_session, restore_checkpoint, rewind_session,
    send_prompt, session_history,
};
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ConfirmDialog, Dropdown, Input, MenuItem, Sheet, SheetBody, SheetHeader,
    SheetTitle, Skeleton, TEXTAREA_CLASS,
};
use crate::dto::{CheckpointDto, HistoryMessageDto, HistoryPageDto};
use crate::live::{LiveKind, LiveTranscript, PendingPermission};
use crate::markdown::Markdown;

const PAGE_SIZE: i64 = 100;

/// Inline error box — persistent until the next action clears it.
const ERROR_BOX: &str =
    "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive";
/// Shared `<pre>` body for tool output and thinking dumps.
const TOOL_PRE: &str = "max-h-80 overflow-auto whitespace-pre-wrap break-words border-t \
                        border-border/60 px-3 py-2 font-mono text-xs text-muted-foreground";

/// `/sessions/:id` — standalone deep link: renders the panel inside
/// the app shell (the same component `/` embeds next to the list).
#[component]
pub fn SessionDetailPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let session_id = move || params.read().get("id").unwrap_or_default();
    let agent = move || non_empty_str(&query.read().get("agent").unwrap_or_default());
    view! {
        {move || {
            view! {
                <SessionPanel session_id=session_id() agent=agent()/>
            }
            .into_any()
        }}
    }
}

fn non_empty_str(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

/// The session chat panel — transcript + prompt + ops. Embedded in the
/// `/` master-detail page (and reachable standalone via the redirect).
#[component]
pub fn SessionPanel(
    #[prop(into)] session_id: Signal<String>,
    #[prop(into)] agent: Signal<Option<String>>,
) -> impl IntoView {
    // Shadow the signals with the closure shape the body was written
    // against — `session_id()`/`agent()` return owned values.
    let session_id = move || session_id.get();
    let agent = move || agent.get();
    // `StoredValue` keeps the navigate fn Copy-able into handlers.
    let navigate = StoredValue::new_local(use_navigate());

    let summary = Resource::new(
        move || (session_id(), agent()),
        |(id, agent)| async move { get_session(id, agent).await },
    );
    let history = Resource::new(
        move || (session_id(), agent()),
        |(id, agent)| async move { session_history(id, agent, None, Some(PAGE_SIZE)).await },
    );
    // Hub outbox rows — filtered to this session for the queued/failed
    // badges. Not session-scoped server-side; the list is small.
    let pending = Resource::new(|| (), |()| pending_writes());

    // Action state lives at page level: a summary refetch re-runs the
    // Suspend subtree, and signals owned inside it would reset.
    let acting = RwSignal::new(false);
    let action_error: RwSignal<Option<String>> = RwSignal::new(None);
    // The summary wire has no `live` flag on plain GETs (it arrives via
    // feed patches), so the attach/detach responses keep a local truth.
    let live_override: RwSignal<Option<bool>> = RwSignal::new(None);
    let rename_draft = RwSignal::new(String::new());
    let confirm_delete = RwSignal::new(false);
    // The right-hand "Details" sheet: rename, checkpoints, danger zone.
    let details_open = RwSignal::new(false);
    let checkpoints_open = Signal::derive(move || details_open.get());
    let checkpoints = Resource::new(
        move || (checkpoints_open.get(), session_id(), agent()),
        |(open, id, agent)| async move {
            if !open || id.is_empty() {
                return Ok(Vec::new());
            }
            list_checkpoints(id, agent).await
        },
    );

    let do_attach = move |takeover: bool| {
        if acting.get() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match attach_session(id, agent, takeover).await {
                Ok(res) => {
                    live_override.set(Some(res.attached));
                    summary.refetch();
                }
                Err(e) => action_error.set(Some(e.to_string())),
            }
            acting.set(false);
        });
    };
    let do_detach = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match detach_session(id, agent).await {
                Ok(()) => {
                    live_override.set(Some(false));
                    summary.refetch();
                }
                Err(e) => action_error.set(Some(e.to_string())),
            }
            acting.set(false);
        });
    };
    let do_cancel = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            if let Err(e) = cancel_run(id, agent).await {
                action_error.set(Some(e.to_string()));
            }
            acting.set(false);
        });
    };
    let do_rename = move || {
        let title = rename_draft.get().trim().to_string();
        if title.is_empty() || acting.get() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match rename_session(id, agent, title).await {
                Ok(()) => {
                    summary.refetch();
                }
                Err(e) => action_error.set(Some(e.to_string())),
            }
            acting.set(false);
        });
    };
    let do_delete = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        action_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match delete_session(id, agent).await {
                Ok(()) => navigate.with_value(|n| n("/", NavigateOptions::default())),
                Err(e) => {
                    action_error.set(Some(e.to_string()));
                    acting.set(false);
                }
            }
        });
    };
    // `ConfirmDialog::on_confirm` requires `Send`, but `do_delete`
    // captures `use_navigate` (the router context holds `!Send`
    // browser handles on wasm), so the dialog flips this flag and the
    // effect performs the delete inside the component owner.
    let delete_confirmed = RwSignal::new(false);
    Effect::new(move |_| {
        if delete_confirmed.get() {
            delete_confirmed.set(false);
            do_delete();
        }
    });

    // Older pages prepended on demand — `before` = the oldest loaded
    // page's `start` index.
    let older: RwSignal<Vec<HistoryPageDto>> = RwSignal::new(Vec::new());
    let live: RwSignal<LiveTranscript> = RwSignal::new(LiveTranscript::default());

    // Live stream — wasm only; SSR renders history without it.
    #[cfg(feature = "hydrate")]
    {
        // `EventStream` isn't `Send` (wasm closures); `new_local` keeps
        // it in the component's arena and its `Drop` closes the stream.
        let _stream = StoredValue::new_local(crate::sse::session_stream(
            &session_id(),
            agent().as_deref(),
            move |event| live.update(|t| t.apply(&event)),
            move || {
                // Lagged — the broadcast ring dropped frames; refetch.
                history.refetch();
                live.set(LiveTranscript::default());
            },
        ));
        // Outbox drain/enqueue emits no feed event — poll slowly.
        crate::app::every_ms(30_000, move || pending.refetch());
    }

    // Auto-scroll the log while new live entries stream in.
    let log_ref = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let _ = live.read().entries.len();
        if let Some(el) = log_ref.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });

    let sending = RwSignal::new(false);
    let send_error: RwSignal<Option<String>> = RwSignal::new(None);
    let draft = RwSignal::new(String::new());

    let submit = move || {
        let text = draft.get().trim().to_string();
        if text.is_empty() || sending.get() {
            return;
        }
        sending.set(true);
        send_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match send_prompt(id, agent, text.clone()).await {
                Ok(()) => {
                    draft.set(String::new());
                    live.update(|t| t.push_user(&text));
                }
                Err(e) => send_error.set(Some(e.to_string())),
            }
            sending.set(false);
        });
    };

    let running = move || live.read().running;
    let latest_start = move || {
        history
            .get()
            .and_then(|p| p.ok().map(|p| p.start))
            .unwrap_or(0)
    };
    let has_older = move || {
        older
            .read()
            .first()
            .map_or_else(|| latest_start() > 0, |p| p.start > 0)
    };

    view! {
        <Title text="session — sepia"/>
        <section class="mx-auto flex h-full w-full max-w-4xl flex-col gap-4 px-4 py-4">
            <Suspense fallback=move || {
                view! {
                    <div class="space-y-3">
                        <Skeleton class="h-32 w-full"/>
                        <Skeleton class="h-72 w-full"/>
                    </div>
                }
            }>
                {move || {
                    Suspend::new(async move {
                        // `summary` the Resource stays reachable for
                        // refetch — the awaited value gets a new name.
                        // A broken outbox read never sinks the page.
                        let (summary_result, page, pending_result) =
                            (summary.await, history.await, pending.await);
                        match (summary_result, page) {
                            (Err(e), _) | (_, Err(e)) => {
                                view! { <p class=ERROR_BOX>{e.to_string()}</p> }.into_any()
                            }
                            (Ok(session), Ok(page)) => {
                                let locked = session.locked;
                                let busy = session.busy;
                                let live_flag = session.live;
                                let title_for_rename = session.title.clone();
                                // Outbox rows for this session — match
                                // the owning node too when both name one
                                // (session ids can collide across nodes).
                                let mut queued = 0usize;
                                let mut failed = 0usize;
                                for w in pending_result.unwrap_or_default() {
                                    let same_node = match (&session.node, w.node_id.is_empty()) {
                                        (Some(n), false) => *n == w.node_id,
                                        _ => true,
                                    };
                                    if w.session_id == session.id && same_node {
                                        if w.status == "failed" {
                                            failed += 1;
                                        } else {
                                            queued += 1;
                                        }
                                    }
                                }
                                view! {
                                    <Card>
                                        <CardHeader class="gap-2">
                                            <A
                                                href="/"
                                                attr:class="inline-flex w-fit items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
                                            >
                                                "← sessions"
                                            </A>
                                            <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                                                <CardTitle class="break-words text-base sm:text-lg">
                                                    {if session.title.trim().is_empty() {
                                                        "Untitled session".to_string()
                                                    } else {
                                                        session.title.clone()
                                                    }}
                                                </CardTitle>
                                                <div class="flex flex-wrap items-center gap-1.5">
                                                    {busy
                                                        .then(|| view! {
                                                            <Badge variant=BadgeVariant::Info>"busy"</Badge>
                                                        })}
                                                    {locked
                                                        .then(|| view! {
                                                            <Badge variant=BadgeVariant::Warning>"locked"</Badge>
                                                        })}
                                                    {(queued > 0).then(|| {
                                                        view! {
                                                            <Badge variant=BadgeVariant::Secondary>
                                                                "queued"
                                                            </Badge>
                                                        }
                                                    })}
                                                    {(failed > 0).then(|| {
                                                        view! {
                                                            <Badge variant=BadgeVariant::Destructive>
                                                                "failed"
                                                            </Badge>
                                                        }
                                                    })}
                                                    {move || {
                                                        live_override
                                                            .get()
                                                            .unwrap_or(live_flag)
                                                            .then(|| view! {
                                                                <Badge variant=BadgeVariant::Success>"live"</Badge>
                                                            })
                                                    }}
                                                </div>
                                            </div>
                                            <CardDescription class="flex flex-wrap items-center gap-2">
                                                <Badge variant=BadgeVariant::Outline>
                                                    {session.agent.clone()}
                                                </Badge>
                                                {session
                                                    .node
                                                    .clone()
                                                    .map(|n| view! {
                                                        <Badge variant=BadgeVariant::Outline>{n}</Badge>
                                                    })}
                                                <code class="break-all font-mono text-xs text-muted-foreground">
                                                    {session.cwd.clone()}
                                                </code>
                                            </CardDescription>
                                        </CardHeader>
                                        <CardContent class="space-y-3">
                                            <div class="flex flex-wrap items-center gap-2">
                                                {move || {
                                                    if live_override.get().unwrap_or(live_flag) {
                                                        view! {
                                                            <Button
                                                                variant=ButtonVariant::Secondary
                                                                size=ButtonSize::Sm
                                                                disabled=acting
                                                                on_click=Box::new(do_detach)
                                                            >
                                                                "Detach"
                                                            </Button>
                                                        }
                                                            .into_any()
                                                    } else if locked {
                                                        view! {
                                                            <Button
                                                                size=ButtonSize::Sm
                                                                disabled=acting
                                                                on_click=Box::new(move || do_attach(true))
                                                            >
                                                                "Attach (takeover)"
                                                            </Button>
                                                        }
                                                            .into_any()
                                                    } else {
                                                        view! {
                                                            <Button
                                                                size=ButtonSize::Sm
                                                                disabled=acting
                                                                on_click=Box::new(move || do_attach(false))
                                                            >
                                                                "Attach"
                                                            </Button>
                                                        }
                                                            .into_any()
                                                    }
                                                }}
                                                {move || {
                                                    (busy || running()).then(|| {
                                                        view! {
                                                            <Button
                                                                variant=ButtonVariant::Outline
                                                                size=ButtonSize::Sm
                                                                disabled=acting
                                                                on_click=Box::new(do_cancel)
                                                            >
                                                                "Cancel run"
                                                            </Button>
                                                        }
                                                    })
                                                }}
                                                <Button
                                                    variant=ButtonVariant::Outline
                                                    size=ButtonSize::Sm
                                                    on_click=Box::new(move || details_open.set(true))
                                                >
                                                    "Details"
                                                </Button>
                                                <Dropdown label="Actions">
                                                    <MenuItem
                                                        label="Rename…"
                                                        on_click=Box::new(move || {
                                                            rename_draft.set(title_for_rename.clone());
                                                            details_open.set(true);
                                                        })
                                                    />
                                                    <MenuItem
                                                        label="Delete session"
                                                        destructive=true
                                                        on_click=Box::new(move || confirm_delete.set(true))
                                                    />
                                                </Dropdown>
                                            </div>
                                            {move || action_error.get().map(|e| {
                                                view! { <p class=ERROR_BOX>{e}</p> }
                                            })}
                                        </CardContent>
                                    </Card>
                                    <ConfirmDialog
                                        open=confirm_delete
                                        title="Delete this session?"
                                        body="This permanently deletes the session and its history from the store."
                                        confirm_label="Delete"
                                        destructive=true
                                        on_confirm=move || delete_confirmed.set(true)
                                    />
                                    <Sheet open=details_open side="right" class="w-96">
                                        <SheetHeader>
                                            <SheetTitle>"Session details"</SheetTitle>
                                            <Button
                                                variant=ButtonVariant::Ghost
                                                size=ButtonSize::Icon
                                                class="size-7"
                                                on_click=Box::new(move || details_open.set(false))
                                            >
                                                "✕"
                                            </Button>
                                        </SheetHeader>
                                        <SheetBody class="space-y-5">
                                            <div class="space-y-2">
                                                <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                                                    "Title"
                                                </label>
                                                <div class="flex gap-2">
                                                    <Input
                                                        class="h-8"
                                                        attr:r#type="text"
                                                        attr:maxlength="200"
                                                        attr:placeholder="Session title"
                                                        prop:value=move || rename_draft.get()
                                                        on:input=move |ev| rename_draft
                                                            .set(event_target_value(&ev))
                                                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                                            if ev.key() == "Enter" {
                                                                ev.prevent_default();
                                                                do_rename();
                                                            }
                                                        }
                                                    />
                                                    <Button
                                                        size=ButtonSize::Sm
                                                        disabled=acting
                                                        on_click=Box::new(do_rename)
                                                    >
                                                        "Save"
                                                    </Button>
                                                </div>
                                            </div>
                                            <CheckpointList
                                                checkpoints=checkpoints
                                                session_id=session_id()
                                                agent=agent()
                                                on_changed=move || {
                                                    history.refetch();
                                                    summary.refetch();
                                                }
                                            />
                                            <div class="rounded-md border border-destructive/40 p-3">
                                                <p class="text-sm font-medium text-destructive">
                                                    "Danger zone"
                                                </p>
                                                <p class="mt-1 text-xs text-muted-foreground">
                                                    "Deleting removes the session and its history."
                                                </p>
                                                <Button
                                                    variant=ButtonVariant::Destructive
                                                    size=ButtonSize::Sm
                                                    class="mt-2"
                                                    disabled=acting
                                                    on_click=Box::new(move || confirm_delete.set(true))
                                                >
                                                    "Delete session"
                                                </Button>
                                            </div>
                                        </SheetBody>
                                    </Sheet>
                                    <div
                                        class="flex min-h-40 flex-1 flex-col gap-2 overflow-y-auto rounded-lg border bg-card p-3"
                                        node_ref=log_ref
                                    >
                                        <OlderButton
                                            older=older
                                            latest_start=latest_start
                                            session_id=session_id()
                                            agent=agent()
                                            has_older=has_older
                                        />
                                        {older
                                            .read()
                                            .iter()
                                            .cloned()
                                            .flat_map(|p| p.messages)
                                            .map(|m| view! { <HistoryRow message=m/> })
                                            .collect::<Vec<_>>()}
                                        {page
                                            .messages
                                            .iter()
                                            .cloned()
                                            .map(|m| view! { <HistoryRow message=m/> })
                                            .collect::<Vec<_>>()}
                                        <LiveLog live=live/>
                                    </div>
                                    {move || {
                                        running()
                                            .then(|| view! {
                                                <p class="flex items-center gap-2 text-sm text-info">
                                                    <span class="size-1.5 animate-pulse rounded-full bg-info"></span>
                                                    "working…"
                                                </p>
                                            })
                                    }}
                                    {move || {
                                        live.read().pending_permission.clone().map(|p| {
                                            view! {
                                                <PermissionCard
                                                    permission=p
                                                    session_id=session_id()
                                                    agent=agent()
                                                    live=live
                                                />
                                            }
                                        })
                                    }}
                                    <PromptBox
                                        draft=draft
                                        sending=sending
                                        send_error=send_error
                                        submit=submit
                                    />
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

/// "Load earlier messages" — fetches the page ending at the current
/// window's `start` index.
#[component]
fn OlderButton(
    older: RwSignal<Vec<HistoryPageDto>>,
    latest_start: impl Fn() -> usize + 'static + Send + Sync + Copy,
    session_id: String,
    agent: Option<String>,
    has_older: impl Fn() -> bool + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let loading = RwSignal::new(false);
    let load = move || {
        let before = older.read().first().map_or_else(latest_start, |p| p.start);
        if before == 0 {
            return;
        }
        loading.set(true);
        let id = session_id.clone();
        let agent = agent.clone();
        leptos::task::spawn_local(async move {
            if let Ok(page) = session_history(
                id,
                agent,
                Some(i64::try_from(before).unwrap_or(i64::MAX)),
                Some(PAGE_SIZE),
            )
            .await
            {
                older.update(|v| v.insert(0, page));
            }
            loading.set(false);
        });
    };
    let load = std::sync::Arc::new(load);
    view! {
        <Show when=move || has_older() fallback=|| ()>
            <div class="flex justify-center">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    disabled=loading
                    on_click=Box::new({
                        let load = load.clone();
                        move || load()
                    })
                >
                    {move || {
                        if loading.get() {
                            "Loading…"
                        } else {
                            "Load earlier messages"
                        }
                    }}
                </Button>
            </div>
        </Show>
    }
}

#[component]
fn HistoryRow(message: HistoryMessageDto) -> impl IntoView {
    let role = message.role.clone();
    let text = message.text();
    if role == "tool" {
        let name = message.tool_name.clone().unwrap_or_else(|| "tool".into());
        let status = message.tool_status.clone().unwrap_or_default();
        let class = if status == "error" {
            "rounded-md border border-destructive/40 bg-destructive/10"
        } else {
            "rounded-md border border-border bg-muted/30"
        };
        view! {
            <details class=class>
                <summary class="flex cursor-pointer select-none items-center gap-2 px-3 py-2 font-mono text-xs">
                    <span class="font-semibold text-info">{name}</span>
                    {message
                        .exit_code
                        .map(|c| view! {
                            <span class="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-secondary-foreground">
                                {format!("exit {c}")}
                            </span>
                        })}
                </summary>
                <pre class=TOOL_PRE>{text}</pre>
            </details>
        }
        .into_any()
    } else {
        let thinking = message.thinking.clone();
        let class = match role.as_str() {
            "user" => "rounded-md border border-info/30 bg-info/5 px-3 py-2",
            "system" => {
                "rounded-md border border-border/60 bg-muted/40 px-3 py-2 text-sm \
                 text-muted-foreground"
            }
            _ => "rounded-md px-1 py-2",
        };
        view! {
            <article class=class>
                <header class="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                    {role.clone()}
                </header>
                {thinking
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        view! {
                            <details class="mb-1 rounded border border-border/60 bg-muted/30">
                                <summary class="cursor-pointer select-none px-2 py-1 font-mono text-xs italic text-muted-foreground">
                                    "thinking"
                                </summary>
                                <pre class=TOOL_PRE>{t}</pre>
                            </details>
                        }
                    })}
                <Markdown text=text/>
            </article>
        }
        .into_any()
    }
}

/// Live entries — streaming text/reasoning and tool calls.
#[component]
fn LiveLog(live: RwSignal<LiveTranscript>) -> impl IntoView {
    view! {
        <div class="contents">
            {move || {
                live.read()
                    .entries
                    .iter()
                    .map(|e| {
                        let surface = if e.error {
                            "border-destructive/50 bg-destructive/10"
                        } else {
                            match e.kind {
                                LiveKind::Assistant => "border-info/30 bg-info/5",
                                LiveKind::Reasoning => {
                                    "border-border/60 bg-muted/40 text-muted-foreground"
                                }
                                LiveKind::Tool => "border-border bg-muted/30",
                            }
                        };
                        let class = format!("rounded-md border px-3 py-2 {surface}");
                        let body = if e.kind == LiveKind::Tool {
                            view! {
                                <details
                                    class="mt-1 rounded border border-border/60 bg-background/60"
                                    open=!e.done
                                >
                                    <summary class="cursor-pointer select-none px-2 py-1 font-mono text-xs text-muted-foreground">
                                        {e.title.clone()}
                                    </summary>
                                    <pre class=TOOL_PRE>
                                        {if e.text.is_empty() {
                                            e.result.clone().unwrap_or_default()
                                        } else {
                                            e.text.clone()
                                        }}
                                    </pre>
                                </details>
                            }
                                .into_any()
                        } else {
                            view! { <Markdown text=e.text.clone()/> }.into_any()
                        };
                        view! {
                            <article class=class>
                                <header class="mb-1 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                                    {(!e.done).then(|| view! {
                                        <span class="size-1.5 animate-pulse rounded-full bg-info"></span>
                                    })}
                                    {e.title.clone()}
                                    {e.error.then(|| view! {
                                        <Badge variant=BadgeVariant::Destructive>"error"</Badge>
                                    })}
                                </header>
                                {body}
                            </article>
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        </div>
    }
}

#[component]
fn PromptBox(
    draft: RwSignal<String>,
    sending: RwSignal<bool>,
    send_error: RwSignal<Option<String>>,
    submit: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    // Enter sends; Shift+Enter inserts a newline.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            submit();
        }
    };
    view! {
        <div class="space-y-2">
            {move || send_error.get().map(|e| view! { <p class=ERROR_BOX>{e}</p> })}
            <textarea
                class=TEXTAREA_CLASS
                placeholder="Message the agent…  (Enter to send, Shift+Enter for newline)"
                prop:value=move || draft.get()
                on:input=move |ev| draft.set(event_target_value(&ev))
                on:keydown=on_keydown
                rows=3
            ></textarea>
            <div class="flex justify-end">
                <Button
                    size=ButtonSize::Sm
                    disabled=move || sending.get() || draft.read().trim().is_empty()
                    on_click=Box::new(submit)
                >
                    {move || if sending.get() { "Sending…" } else { "Send" }}
                </Button>
            </div>
        </div>
    }
}

/// The `acp:permission_request` card — one button per option plus a
/// dismiss (`optionId: null` settles the request without an option).
/// Answering clears the pending request out of the live transcript.
#[component]
fn PermissionCard(
    permission: PendingPermission,
    session_id: String,
    agent: Option<String>,
    live: RwSignal<LiveTranscript>,
) -> impl IntoView {
    let answering = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let request_id = permission.request_id.clone();
    let respond = move |option_id: Option<String>| {
        if answering.get() {
            return;
        }
        answering.set(true);
        error.set(None);
        let id = session_id.clone();
        let agent = agent.clone();
        let request_id = request_id.clone();
        leptos::task::spawn_local(async move {
            match answer_permission(id, agent, request_id, option_id).await {
                Ok(()) => live.update(|t| t.pending_permission = None),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    answering.set(false);
                }
            }
        });
    };
    view! {
        <Card class="border-warning/40">
            <CardHeader class="gap-2">
                <div class="flex items-center gap-2">
                    <Badge variant=BadgeVariant::Warning>"approval"</Badge>
                    <CardTitle class="text-sm font-medium">
                        {if permission.title.is_empty() {
                            "Permission requested".to_string()
                        } else {
                            permission.title.clone()
                        }}
                    </CardTitle>
                </div>
                {move || error.get().map(|e| view! { <p class=ERROR_BOX>{e}</p> })}
            </CardHeader>
            <CardContent>
                <div class="flex flex-wrap items-center gap-2">
                    {permission
                        .options
                        .iter()
                        .map(|o| {
                            let o = o.clone();
                            let respond = respond.clone();
                            let (variant, extra) = if o.kind.contains("reject") {
                                (
                                    ButtonVariant::Outline,
                                    "border-destructive/40 text-destructive hover:bg-destructive/10",
                                )
                            } else if o.kind.contains("always") {
                                (ButtonVariant::Default, "")
                            } else {
                                (ButtonVariant::Secondary, "")
                            };
                            view! {
                                <Button
                                    variant=variant
                                    size=ButtonSize::Sm
                                    class=extra.to_string()
                                    disabled=answering
                                    on_click=Box::new(move || respond(Some(o.option_id.clone())))
                                >
                                    {o.name.clone()}
                                </Button>
                            }
                        })
                        .collect::<Vec<_>>()}
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        disabled=answering
                        on_click=Box::new(move || respond(None))
                    >
                        "Dismiss"
                    </Button>
                </div>
            </CardContent>
        </Card>
    }
}

/// The checkpoints panel — `GET /api/sessions/{id}/checkpoints` fetched
/// lazily by the resource's `open` trigger.
#[component]
fn CheckpointList(
    checkpoints: Resource<Result<Vec<CheckpointDto>, ServerFnError>>,
    session_id: String,
    agent: Option<String>,
    on_changed: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle class="text-sm">"Checkpoints"</CardTitle>
                <CardDescription>
                    "Restore the store state or rewind the transcript to a recorded checkpoint."
                </CardDescription>
            </CardHeader>
            <CardContent>
                {move || match checkpoints.get() {
                    None => {
                        view! {
                            <div class="space-y-2">
                                <Skeleton class="h-20 w-full"/>
                                <Skeleton class="h-20 w-full"/>
                            </div>
                        }
                            .into_any()
                    }
                    Some(Err(e)) => view! { <p class=ERROR_BOX>{e.to_string()}</p> }.into_any(),
                    Some(Ok(list)) if list.is_empty() => {
                        view! {
                            <p class="text-sm text-muted-foreground">
                                "No checkpoints recorded."
                            </p>
                        }
                            .into_any()
                    }
                    Some(Ok(list)) => {
                        view! {
                            <ul class="space-y-2">
                                {list
                                    .into_iter()
                                    .map(|cp| {
                                        view! {
                                            <CheckpointRow
                                                checkpoint=cp
                                                session_id=session_id.clone()
                                                agent=agent.clone()
                                                on_changed=on_changed
                                            />
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </ul>
                        }
                            .into_any()
                    }
                }}
            </CardContent>
        </Card>
    }
}

/// One checkpoint row. `Restore`/`Rewind` ask for confirmation via a
/// `ConfirmDialog` before posting.
#[component]
fn CheckpointRow(
    checkpoint: CheckpointDto,
    session_id: String,
    agent: Option<String>,
    on_changed: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let done: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let confirm_restore = RwSignal::new(false);
    let confirm_rewind = RwSignal::new(false);
    let checkpoint_ref = checkpoint.r#ref.clone();
    let run = move |op: &'static str| {
        if busy.get() {
            return;
        }
        busy.set(true);
        error.set(None);
        done.set(None);
        let id = session_id.clone();
        let agent = agent.clone();
        let checkpoint = checkpoint_ref.clone();
        leptos::task::spawn_local(async move {
            let result = if op == "restore" {
                restore_checkpoint(id, agent, checkpoint).await
            } else {
                rewind_session(id, agent, checkpoint).await
            };
            match result {
                Ok(()) => {
                    done.set(Some(if op == "restore" {
                        "Restored."
                    } else {
                        "Rewound."
                    }));
                    on_changed();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let run_restore = {
        let run = run.clone();
        move || run("restore")
    };
    let run_rewind = move || run("rewind");
    view! {
        <li class="space-y-2 rounded-md border border-border/60 bg-background/40 p-3">
            <div class="flex flex-wrap items-center gap-2">
                <code class="rounded bg-muted px-1.5 py-0.5 font-mono text-xs text-info">
                    {checkpoint.r#ref.clone()}
                </code>
                {checkpoint
                    .kind
                    .clone()
                    .map(|k| view! { <Badge variant=BadgeVariant::Info>{k}</Badge> })}
                {checkpoint
                    .run_count
                    .map(|n| view! {
                        <Badge variant=BadgeVariant::Muted>{format!("{n} runs")}</Badge>
                    })}
                <span class="ml-auto text-xs text-muted-foreground">
                    {move || {
                        let now = use_context::<crate::app::Now>()
                            .map_or_else(crate::time::now_ms, |n| n.0.get());
                        crate::time::relative_ms(checkpoint.created_at, now)
                    }}
                </span>
            </div>
            {move || error.get().map(|e| view! { <p class=ERROR_BOX>{e}</p> })}
            {move || done.get().map(|d| view! { <p class="text-sm text-success">{d}</p> })}
            <div class="flex items-center gap-2">
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    disabled=busy
                    on_click=Box::new(move || confirm_restore.set(true))
                >
                    {move || if busy.get() { "Working…" } else { "Restore" }}
                </Button>
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    class="border-warning/40 text-warning hover:bg-warning/10".to_string()
                    disabled=busy
                    on_click=Box::new(move || confirm_rewind.set(true))
                >
                    {move || if busy.get() { "Working…" } else { "Rewind" }}
                </Button>
            </div>
            <ConfirmDialog
                open=confirm_restore
                title="Restore this checkpoint?"
                body="The store state is reset to this checkpoint; the transcript is unchanged."
                confirm_label="Restore"
                on_confirm=run_restore
            />
            <ConfirmDialog
                open=confirm_rewind
                title="Rewind to this checkpoint?"
                body="History after this checkpoint is removed from the transcript."
                confirm_label="Rewind"
                destructive=true
                on_confirm=run_rewind
            />
        </li>
    }
}
