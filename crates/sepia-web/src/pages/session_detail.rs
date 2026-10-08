//! `/sessions/:id` — history (paged via `?before`), live SSE stream
//! folded into a transcript, and the prompt box.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::api::{get_session, send_prompt, session_history};
use crate::dto::{HistoryMessageDto, HistoryPageDto};
use crate::live::{LiveKind, LiveTranscript};
use crate::markdown::Markdown;

const PAGE_SIZE: i64 = 100;

#[component]
pub fn SessionDetailPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let session_id = move || params.read().get("id").unwrap_or_default();
    let agent = move || non_empty(&query.read().get("agent").unwrap_or_default());

    let summary = Resource::new(
        move || (session_id(), agent()),
        |(id, agent)| async move { get_session(id, agent).await },
    );
    let history = Resource::new(
        move || (session_id(), agent()),
        |(id, agent)| async move { session_history(id, agent, None, Some(PAGE_SIZE)).await },
    );

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
                        let (summary, page) = (summary.await, history.await);
                        match (summary, page) {
                            (Err(e), _) | (_, Err(e)) => {
                                view! { <p class="error">{e.to_string()}</p> }.into_any()
                            }
                            (Ok(session), Ok(page)) => {
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
                                            {session
                                                .busy
                                                .then(|| view! { <span class="badge busy">"busy"</span> })}
                                            {session
                                                .locked
                                                .then(|| view! { <span class="badge locked">"locked"</span> })}
                                        </span>
                                        <p class="detail-meta">
                                            <span class="agent">{session.agent.clone()}</span>
                                            <code class="cwd">{session.cwd.clone()}</code>
                                        </p>
                                    </header>
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
