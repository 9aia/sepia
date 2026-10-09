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
use crate::dto::{CheckpointDto, HistoryMessageDto, HistoryPageDto};
use crate::live::{LiveKind, LiveTranscript, PendingPermission};
use crate::markdown::Markdown;

const PAGE_SIZE: i64 = 100;

#[component]
pub fn SessionDetailPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let session_id = move || params.read().get("id").unwrap_or_default();
    let agent = move || non_empty(&query.read().get("agent").unwrap_or_default());
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
    let renaming = RwSignal::new(false);
    let rename_draft = RwSignal::new(String::new());
    let confirm_delete = RwSignal::new(false);
    let checkpoints_open = RwSignal::new(false);
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
                    renaming.set(false);
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
        // Two-step confirm — one click arms, the second deletes.
        if !confirm_delete.get() {
            confirm_delete.set(true);
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
                    confirm_delete.set(false);
                    acting.set(false);
                }
            }
        });
    };

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
        <section class="page detail">
            <Suspense fallback=move || {
                view! { <p class="loading">"Loading session…"</p> }
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
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
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
                                    <header class="detail-head">
                                        <A href="/" attr:class="back">"← sessions"</A>
                                        <h1 class="detail-title">
                                            {if session.title.trim().is_empty() {
                                                "Untitled session".to_string()
                                            } else {
                                                session.title.clone()
                                            }}
                                        </h1>
                                        <span class="badges">
                                            {busy
                                                .then(|| view! { <span class="badge busy">"busy"</span> })}
                                            {locked
                                                .then(|| view! { <span class="badge locked">"locked"</span> })}
                                            {(queued > 0).then(|| {
                                                view! { <span class="badge queued">"queued"</span> }
                                            })}
                                            {(failed > 0).then(|| {
                                                view! { <span class="badge failed">"failed"</span> }
                                            })}
                                            {move || {
                                                live_override
                                                    .get()
                                                    .unwrap_or(live_flag)
                                                    .then(|| view! { <span class="badge live">"live"</span> })
                                            }}
                                        </span>
                                        <p class="detail-meta">
                                            <span class="agent">{session.agent.clone()}</span>
                                            <code class="cwd">{session.cwd.clone()}</code>
                                        </p>
                                        <div class="actions">
                                            {move || {
                                                if live_override.get().unwrap_or(live_flag) {
                                                    view! {
                                                        <button
                                                            class="action"
                                                            disabled=move || acting.get()
                                                            on:click=move |_| do_detach()
                                                        >
                                                            "Detach"
                                                        </button>
                                                    }
                                                        .into_any()
                                                } else if locked {
                                                    view! {
                                                        <button
                                                            class="action"
                                                            disabled=move || acting.get()
                                                            on:click=move |_| do_attach(true)
                                                        >
                                                            "Attach (takeover)"
                                                        </button>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            class="action"
                                                            disabled=move || acting.get()
                                                            on:click=move |_| do_attach(false)
                                                        >
                                                            "Attach"
                                                        </button>
                                                    }
                                                        .into_any()
                                                }
                                            }}
                                            {move || {
                                                (busy || running()).then(|| {
                                                    view! {
                                                        <button
                                                            class="action"
                                                            disabled=move || acting.get()
                                                            on:click=move |_| do_cancel()
                                                        >
                                                            "Cancel run"
                                                        </button>
                                                    }
                                                })
                                            }}
                                            <button
                                                class="action"
                                                disabled=move || acting.get()
                                                on:click=move |_| {
                                                    rename_draft.set(title_for_rename.clone());
                                                    renaming.set(true);
                                                }
                                            >
                                                "Rename"
                                            </button>
                                            <button
                                                class="action"
                                                on:click=move |_| checkpoints_open.update(|o| *o = !*o)
                                            >
                                                {move || {
                                                    if checkpoints_open.get() {
                                                        "Hide checkpoints"
                                                    } else {
                                                        "Checkpoints"
                                                    }
                                                }}
                                            </button>
                                            <button
                                                class="danger"
                                                disabled=move || acting.get()
                                                on:click=move |_| do_delete()
                                            >
                                                {move || {
                                                    if confirm_delete.get() {
                                                        "Confirm delete"
                                                    } else {
                                                        "Delete"
                                                    }
                                                }}
                                            </button>
                                        </div>
                                        {move || renaming.get().then(|| {
                                            view! {
                                                <div class="form-row rename-row">
                                                    <input
                                                        class="field"
                                                        type="text"
                                                        maxlength=200
                                                        placeholder="Session title"
                                                        prop:value=move || rename_draft.get()
                                                        on:input=move |ev| rename_draft
                                                            .set(event_target_value(&ev))
                                                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                                            if ev.key() == "Enter" {
                                                                ev.prevent_default();
                                                                do_rename();
                                                            } else if ev.key() == "Escape" {
                                                                renaming.set(false);
                                                            }
                                                        }
                                                    />
                                                    <button
                                                        class="save small"
                                                        disabled=move || acting.get()
                                                        on:click=move |_| do_rename()
                                                    >
                                                        "Save"
                                                    </button>
                                                    <button
                                                        class="action"
                                                        on:click=move |_| renaming.set(false)
                                                    >
                                                        "Cancel"
                                                    </button>
                                                </div>
                                            }
                                        })}
                                        {move || action_error.get().map(|e| {
                                            view! { <p class="error">{e}</p> }
                                        })}
                                    </header>
                                    <Show when=move || checkpoints_open.get() fallback=|| ()>
                                        <CheckpointList
                                            checkpoints=checkpoints
                                            session_id=session_id()
                                            agent=agent()
                                            on_changed=move || {
                                                history.refetch();
                                                summary.refetch();
                                            }
                                        />
                                    </Show>
                                    <div class="log" node_ref=log_ref>
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
                                            .then(|| view! { <p class="busy-line">"working…"</p> })
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

fn non_empty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
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
    let onclick = move |_| {
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
    view! {
        <Show when=move || has_older() fallback=|| ()>
            <button class="older" disabled=move || loading.get() on:click=onclick.clone()>
                {move || {
                    if loading.get() {
                        "Loading…"
                    } else {
                        "Load earlier messages"
                    }
                }}
            </button>
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
            "msg tool error"
        } else {
            "msg tool"
        };
        view! {
            <details class=class>
                <summary>
                    <span class="tool-name">{name}</span>
                    {message
                        .exit_code
                        .map(|c| view! { <span class="tool-code">{format!("exit {c}")}</span> })}
                </summary>
                <pre class="tool-body">{text}</pre>
            </details>
        }
        .into_any()
    } else {
        let thinking = message.thinking.clone();
        view! {
            <article class=format!("msg {role}")>
                <header class="msg-role">{role.clone()}</header>
                {thinking
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        view! {
                            <details class="thinking">
                                <summary>"thinking"</summary>
                                <pre class="tool-body">{t}</pre>
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
        <div class="live">
            {move || {
                live.read()
                    .entries
                    .iter()
                    .map(|e| {
                        let class = format!(
                            "msg live-{}{}{}",
                            match e.kind {
                                LiveKind::Assistant => "assistant",
                                LiveKind::Reasoning => "reasoning",
                                LiveKind::Tool => "tool",
                            },
                            if e.done { " done" } else { "" },
                            if e.error { " error" } else { "" },
                        );
                        let body = if e.kind == LiveKind::Tool {
                            view! {
                                <details class="tool-live" open=!e.done>
                                    <summary>{e.title.clone()}</summary>
                                    <pre class="tool-body">
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
                                <header class="msg-role">{e.title.clone()}</header>
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
        <div class="composer">
            {move || send_error.get().map(|e| view! { <p class="error">{e}</p> })}
            <textarea
                class="prompt"
                placeholder="Message the agent…  (Enter to send, Shift+Enter for newline)"
                prop:value=move || draft.get()
                on:input=move |ev| draft.set(event_target_value(&ev))
                on:keydown=on_keydown
                rows=3
            ></textarea>
            <div class="composer-bar">
                <button
                    class="send"
                    disabled=move || sending.get() || draft.read().trim().is_empty()
                    on:click=move |_| submit()
                >
                    {move || if sending.get() { "Sending…" } else { "Send" }}
                </button>
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
        <div class="permission">
            <p class="permission-title">
                <span class="badge locked">"approval"</span>
                {if permission.title.is_empty() {
                    "Permission requested".to_string()
                } else {
                    permission.title.clone()
                }}
            </p>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            <div class="permission-options">
                {permission
                    .options
                    .iter()
                    .map(|o| {
                        let o = o.clone();
                        let respond = respond.clone();
                        view! {
                            <button
                                class=format!("perm-option {}", o.kind)
                                disabled=move || answering.get()
                                on:click=move |_| respond(Some(o.option_id.clone()))
                            >
                                {o.name.clone()}
                            </button>
                        }
                    })
                    .collect::<Vec<_>>()}
                <button
                    class="action"
                    disabled=move || answering.get()
                    on:click=move |_| respond(None)
                >
                    "Dismiss"
                </button>
            </div>
        </div>
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
        <div class="checkpoints">
            {move || match checkpoints.get() {
                None => view! { <p class="loading">"Loading checkpoints…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{e.to_string()}</p> }.into_any(),
                Some(Ok(list)) if list.is_empty() => {
                    view! { <p class="empty">"No checkpoints recorded."</p> }.into_any()
                }
                Some(Ok(list)) => {
                    view! {
                        <ul class="checkpoint-list">
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
        </div>
    }
}

/// One checkpoint row. `Restore`/`Rewind` are two-step — the first
/// click arms ("Confirm"), the second posts with `confirm: true`.
#[component]
fn CheckpointRow(
    checkpoint: CheckpointDto,
    session_id: String,
    agent: Option<String>,
    on_changed: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let armed: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let done: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let checkpoint_ref = checkpoint.r#ref.clone();
    let run = move |op: &'static str| {
        if busy.get() {
            return;
        }
        if armed.get() != Some(op) {
            armed.set(Some(op));
            return;
        }
        armed.set(None);
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
    view! {
        <li class="checkpoint">
            <div class="checkpoint-head">
                <code class="checkpoint-ref">{checkpoint.r#ref.clone()}</code>
                <span class="badges">
                    {checkpoint.kind.clone().map(|k| view! { <span class="badge">{k}</span> })}
                    {checkpoint
                        .run_count
                        .map(|n| view! { <span class="badge">{format!("{n} runs")}</span> })}
                </span>
                <span class="checkpoint-time">
                    {move || {
                        let now = use_context::<crate::app::Now>()
                            .map_or_else(crate::time::now_ms, |n| n.0.get());
                        crate::time::relative_ms(checkpoint.created_at, now)
                    }}
                </span>
            </div>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            {move || done.get().map(|d| view! { <p class="ok-line">{d}</p> })}
            <div class="card-actions">
                <button
                    class="action"
                    disabled=move || busy.get()
                    on:click={
                        let run = run.clone();
                        move |_| run("restore")
                    }
                >
                    {move || {
                        if busy.get() {
                            "Working…"
                        } else if armed.get() == Some("restore") {
                            "Confirm restore"
                        } else {
                            "Restore"
                        }
                    }}
                </button>
                <button
                    class="action"
                    disabled=move || busy.get()
                    on:click=move |_| run("rewind")
                >
                    {move || {
                        if busy.get() {
                            "Working…"
                        } else if armed.get() == Some("rewind") {
                            "Confirm rewind"
                        } else {
                            "Rewind"
                        }
                    }}
                </button>
            </div>
        </li>
    }
}
