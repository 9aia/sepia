//! `/sessions/:id` — history (paged via `?before`), live SSE stream
//! folded into a transcript, and the prompt box.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
#[cfg(feature = "hydrate")]
use leptos_router::hooks::use_navigate;
use leptos_router::hooks::{use_params_map, use_query_map};
use leptos_use::core::ConnectionReadyState;
use sepia_web_core::history::{
    ChatRow, FileDiffView, RowInput, RunSpan, SystemContext, ToolSegment, build_rows,
    file_diff_view, format_duration, format_usage, tool_content_segments, tool_summary,
    usage_label,
};
use sepia_web_core::transcript::{LiveKind, LiveTranscript, PendingPermission};
use serde_json::Value;

use crate::api::{
    answer_permission, attach_session, cancel_run, delete_session, detach_session, rename_session,
    restore_checkpoint, rewind_session, send_prompt, session_history,
};
use crate::components::icons::Icon;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ConfirmDialog, Dropdown, ErrorBanner, Input, MenuItem, Sheet, SheetBody,
    SheetHeader, SheetTitle, Skeleton, TEXTAREA_CLASS,
};
use crate::dto::{CheckpointDto, HistoryMessageDto, HistoryPageDto};
use crate::markdown::Markdown;

const PAGE_SIZE: i64 = 100;

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

/// `- `/`+ `/`⋮ ` prefixed diff lines → colored rows. `added`/`removed`
/// count the real change when the text came from `file_diff_view`; a
/// plain `-/+` block (args-derived diffs) passes `None`.
#[component]
fn DiffBlock(
    #[prop(into)] text: String,
    #[prop(optional_no_strip)] path: Option<String>,
    #[prop(optional)] added: Option<usize>,
    #[prop(optional)] removed: Option<usize>,
) -> impl IntoView {
    let lines = text
        .lines()
        .map(|line| {
            let cls = if line.starts_with("+ ") || line == "+" {
                "text-success bg-success/10"
            } else if line.starts_with("- ") || line == "-" {
                "text-destructive bg-destructive/10"
            } else if line.starts_with('⋮') {
                "italic text-muted-foreground"
            } else {
                "text-muted-foreground"
            };
            view! { <div class=format!("px-2 whitespace-pre-wrap break-words {cls}")>{line.to_string()}</div> }
        })
        .collect::<Vec<_>>();
    view! {
        <div data-name="DiffBlock" class="overflow-hidden border-t border-border/60">
            {path.map(|p| {
                view! {
                    <div class="flex items-center gap-2 border-b border-border/60 bg-muted/40 px-2.5 py-1 font-mono text-[10px] text-muted-foreground">
                        <span class="min-w-0 flex-1 truncate">{p}</span>
                        {added.map(|a| view! { <span class="shrink-0 text-success">{format!("+{a}")}</span> })}
                        {removed.map(|r| view! { <span class="shrink-0 text-destructive">{format!("−{r}")}</span> })}
                    </div>
                }
            })}
            <div class="max-h-72 overflow-auto py-1 font-mono text-[11px] leading-relaxed">{lines}</div>
        </div>
    }
}

/// One typed body segment of a tool display — command line, code dump,
/// inline diff, markdown, or a muted status note.
fn tool_segment_view(segment: ToolSegment) -> impl IntoView {
    match segment {
        ToolSegment::Command(text) => view! {
            <div class="flex items-start gap-1.5 border-t border-border/60 px-3 py-1.5 font-mono text-[11px]">
                <span class="shrink-0 select-none text-muted-foreground/70">"$"</span>
                <span class="min-w-0 whitespace-pre-wrap break-words text-foreground/80">{text}</span>
            </div>
        }
        .into_any(),
        ToolSegment::Code(text) => view! {
            <pre class=TOOL_PRE>{text}</pre>
        }
        .into_any(),
        ToolSegment::Diff(text) => view! {
            <DiffBlock text=text/>
        }
        .into_any(),
        ToolSegment::Markdown(text) => view! {
            <div class="border-t border-border/60 px-3 py-1"><Markdown text=text/></div>
        }
        .into_any(),
        ToolSegment::Note { text, error } => {
            let cls = if error {
                "text-destructive"
            } else {
                "text-muted-foreground/80"
            };
            view! {
                <p class=format!("border-t border-border/60 px-3 py-1 text-[11px] {cls}")>{text}</p>
            }
            .into_any()
        }
    }
}

/// The collapsible tool row shared by history and live entries: a
/// marker line (label + status + salient detail) over typed segments,
/// recorded file diffs, touched-file locations, and live `contents`.
#[allow(clippy::too_many_arguments)]
#[component]
fn ToolBlock(
    /// The `tool_summary` result for this call.
    display: sepia_web_core::history::ToolDisplay,
    /// `"running…" | "done" | "error" | "pending" | "success" | "failed"`.
    #[prop(into)]
    status: String,
    /// Open while streaming; settled rows collapse via native
    /// `<details>` toggling (`open` keeps SSR/hydrate markup identical).
    open: bool,
    /// Degraded-surface border (`status == error` or a failed run).
    error: bool,
    /// Recorded before/after payloads → real diff blocks.
    #[prop(into, optional)]
    diffs: Vec<FileDiffView>,
    /// Files the call touched, `path[:line]` — shown when present even
    /// alongside diffs (the wire sends both).
    #[prop(into, optional)]
    locations: Vec<String>,
    /// Extra live `contents` segments (terminal refs, embedded text).
    #[prop(into, optional)]
    contents: Vec<ToolSegment>,
    /// `durationMs` chip on the marker line.
    #[prop(optional_no_strip)]
    duration_ms: Option<f64>,
) -> impl IntoView {
    let class = if error {
        "rounded-md border border-destructive/40 bg-destructive/10"
    } else {
        "rounded-md border border-border bg-muted/40"
    };
    let status_cls = if error || status == "error" || status == "failed" {
        "text-destructive"
    } else {
        "text-muted-foreground"
    };
    let detail = display.detail.clone().filter(|d| !d.is_empty());
    let running = status == "running…";
    let empty = display.segments.is_empty()
        && diffs.is_empty()
        && locations.is_empty()
        && contents.is_empty();
    view! {
        <details class=class open=open data-name=display.category.hook()>
            <summary class="flex cursor-pointer select-none items-center gap-2 px-3 py-2 font-mono text-xs">
                {running.then(|| view! {
                    <span class="size-1.5 shrink-0 animate-pulse rounded-full bg-info"></span>
                })}
                <span class="shrink-0 font-semibold text-info">{display.label.clone()}</span>
                {(!status.is_empty()).then(move || view! {
                    <span class=format!("shrink-0 text-[10px] uppercase tracking-wide {status_cls}")>
                        {status.clone()}
                    </span>
                })}
                {duration_ms
                    .map(format_duration)
                    .filter(|d| !d.is_empty())
                    .map(|d| view! {
                        <span class="shrink-0 text-[10px] text-muted-foreground">{format!("({d})")}</span>
                    })}
                {detail.map(|d| {
                    let label = d.clone();
                    view! {
                        <span class="min-w-0 flex-1 truncate text-muted-foreground" title=d>{label}</span>
                    }
                })}
            </summary>
            {display
                .segments
                .into_iter()
                .map(tool_segment_view)
                .collect::<Vec<_>>()}
            {diffs
                .into_iter()
                .map(|d| {
                    view! {
                        <DiffBlock
                            text=d.text
                            path=Some(d.path)
                            added=d.added
                            removed=d.removed
                        />
                    }
                })
                .collect::<Vec<_>>()}
            {(!locations.is_empty()).then(|| {
                view! {
                    <div class="flex flex-wrap items-center gap-1.5 border-t border-border/60 px-3 py-1.5">
                        {locations
                            .iter()
                            .map(|loc| {
                                view! {
                                    <code class="truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground" title=loc.clone()>
                                        {loc.clone()}
                                    </code>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                }
            })}
            {contents.into_iter().map(tool_segment_view).collect::<Vec<_>>()}
            {empty.then(|| view! {
                <p class="border-t border-border/60 px-3 py-2 text-xs italic text-muted-foreground">
                    "no output"
                </p>
            })}
        </details>
    }
}

/// Slim divider between transcript segments — run-span provenance
/// (`agent @ node`), model switches (`model: X`), and live run
/// boundaries (`run started`/`run ended`).
#[component]
fn RunMarker(#[prop(into)] label: String) -> impl IntoView {
    view! {
        <div data-name="RunMarker" class="flex items-center gap-2 px-1 py-0.5">
            <span class="h-px flex-1 bg-border/60"></span>
            <span class="shrink-0 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                {label}
            </span>
            <span class="h-px flex-1 bg-border/60"></span>
        </div>
    }
}

/// Non-text `blocks` entries — an image inlines, anything else (file,
/// audio, uri-less image) collapses to a named chip.
fn attachment_views(blocks: &[Value]) -> Vec<impl IntoView> {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) != Some("text"))
        .map(|b| {
            let ty = b.get("type").and_then(Value::as_str).unwrap_or("file");
            if ty == "image" {
                let src = b
                    .get("data")
                    .and_then(Value::as_str)
                    .map(|d| {
                        let mime = b
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png");
                        format!("data:{mime};base64,{d}")
                    })
                    .or_else(|| b.get("uri").and_then(Value::as_str).map(str::to_string));
                if let Some(src) = src {
                    let alt = b
                        .get("uri")
                        .and_then(Value::as_str)
                        .or_else(|| b.get("mimeType").and_then(Value::as_str))
                        .unwrap_or("image")
                        .to_string();
                    return view! {
                        <img src=src alt=alt class="mt-1.5 max-h-72 rounded-md border border-border/60"/>
                    }
                    .into_any();
                }
            }
            let name = b
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| b.get("uri").and_then(Value::as_str))
                .unwrap_or(ty)
                .to_string();
            let detail = b
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    b.get("size").and_then(Value::as_f64).map(|bytes| {
                        if bytes < 1024.0 {
                            format!("{bytes:.0} B")
                        } else if bytes < 1024.0 * 1024.0 {
                            format!("{:.1} KB", bytes / 1024.0)
                        } else {
                            format!("{:.1} MB", bytes / (1024.0 * 1024.0))
                        }
                    })
                })
                .unwrap_or_default();
            view! {
                <span class="mt-1.5 inline-flex items-center gap-1.5 rounded-md border border-border/60 bg-muted/40 px-2 py-1 text-xs text-muted-foreground">
                    {name}
                    {(!detail.is_empty()).then(|| view! {
                        <span class="text-muted-foreground/70">{detail.clone()}</span>
                    })}
                </span>
            }
            .into_any()
        })
        .collect()
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
    // `new_local` stores a SendWrapper in the owner arena — on SSR the
    // owner cleans up on an arbitrary tokio worker and the guard
    // panics, aborting the response stream. SSR never navigates.
    #[cfg(feature = "hydrate")]
    let navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});

    let client = crate::api::query_client();
    let summary = client.resource(crate::api::session_scope, move || (session_id(), agent()));
    let history = client.resource(crate::api::history_scope, move || (session_id(), agent()));
    // Hub outbox rows — filtered to this session for the queued/failed
    // badges. Not session-scoped server-side; the list is small.
    let pending = client.resource(crate::api::pending_scope, || ());

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
    let checkpoints = client.resource(crate::api::checkpoints_scope, move || {
        (checkpoints_open.get(), session_id(), agent())
    });

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
                    client.invalidate_query(crate::api::sessions_scope, ());
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
                    client.invalidate_query(crate::api::sessions_scope, ());
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
                    client.invalidate_query(crate::api::sessions_scope, ());
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
                Ok(()) => {
                    client.invalidate_query(crate::api::sessions_scope, ());
                    navigate.with_value(|n| n("/", NavigateOptions::default()));
                }
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
    // The live stream's connection state — `Some(signal)` while an
    // EventStream is open, `None` otherwise (SSR, detached, or before
    // the summary resolves). The `Reconnecting…` pill reads it.
    let conn_state: RwSignal<Option<Signal<ConnectionReadyState>>> = RwSignal::new(None);

    // Auto-scroll the log while new live entries stream in — but only
    // while the user is pinned to the bottom. Scrolling up unpins; the
    // "jump to bottom" pill re-pins. `log_ref`/`pinned` live ahead of
    // the history loader because the scroll-top auto-load and the
    // prepend scroll-restore both touch them.
    let log_ref = NodeRef::<leptos::html::Div>::new();
    let pinned = RwSignal::new(true);

    // Earlier-history paging — shared by the "Load earlier" button and
    // the scroll-top trigger (the same guard gates both).
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
    let older_loading = RwSignal::new(false);
    // scrollHeight captured before a prepend resolves — the restore
    // effect shifts scrollTop by the grown amount so prepends don't
    // jump the viewport. Hydrate-only; the SSR build never touches it.
    #[allow(unused_variables)]
    let prepend_height = StoredValue::new(0.0_f64);
    let load_older = move || {
        if older_loading.get_untracked() {
            return;
        }
        let before = older.read().first().map_or_else(latest_start, |p| p.start);
        if before == 0 {
            return;
        }
        older_loading.set(true);
        #[cfg(feature = "hydrate")]
        if let Some(el) = log_ref.get() {
            prepend_height.set_value(f64::from(el.scroll_height()));
        }
        let id = session_id();
        let agent = agent();
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
            older_loading.set(false);
        });
    };

    // Live stream — wasm only; SSR renders history without it.
    #[cfg(feature = "hydrate")]
    {
        // `agent` comes from the `?agent=` param, but bare `/?session=`
        // and `/sessions/:id` links lack it — resolve from the summary
        // as a fallback. And the stream endpoint 400s unless the
        // session is *attached* — open only for live sessions, close on
        // detach. The slot lives in the component owner: values created
        // inside the Effect would be disposed with the effect run while
        // leptos_use's reconnect timer still touches them.
        let slot = StoredValue::new_local(None::<crate::sse::EventStream>);
        Effect::new(move |_| {
            let session = summary.get().and_then(Result::ok);
            let resolved = agent().or_else(|| {
                session
                    .as_ref()
                    .map(|s| s.agent.clone())
                    .filter(|a| !a.is_empty())
            });
            let live_now = live_override.get().or(session.map(|s| s.live));
            match (resolved, live_now) {
                // Attached + agent known → open once.
                (Some(agent_name), Some(true)) if slot.with_value(|s| s.is_none()) => {
                    // `EventStream` isn't `Send` (wasm closures);
                    // `new_local` keeps the slot in the component's
                    // arena and the guard's `Drop` closes the stream.
                    let stream = crate::sse::session_stream(
                        &session_id(),
                        Some(&agent_name),
                        move |event| live.update(|t| t.apply(&event)),
                        move || {
                            // Lagged — the broadcast ring dropped frames.
                            history.refetch();
                            live.set(LiveTranscript::default());
                        },
                    );
                    conn_state.set(Some(stream.ready_state()));
                    slot.set_value(Some(stream));
                }
                // Detached → close an open stream, killing the retry
                // loop against the 400ing endpoint.
                (_, Some(false)) => {
                    slot.set_value(None);
                    conn_state.set(None);
                }
                _ => {}
            }
        });
        // Outbox drain/enqueue emits no feed event — poll slowly.
        crate::app::every_ms(30_000, move || pending.refetch());
    }

    #[cfg(feature = "hydrate")]
    {
        let _cleanup = leptos_use::use_event_listener(log_ref, leptos::ev::scroll, move |ev| {
            use wasm_bindgen::JsCast;
            if let Some(el) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
            {
                // 48px slop — "at bottom" is approximate for streaming rows.
                let top = f64::from(el.scroll_top());
                let gap = f64::from(el.scroll_height()) - top - f64::from(el.client_height());
                pinned.set(gap < 48.0);
                // Scroll-top auto-load — the manual "Load earlier"
                // button stays for a11y; same guard gates both.
                if top < 64.0 && has_older() {
                    load_older();
                }
            }
        });
        Effect::new(move |_| {
            let _ = live.read().entries.len();
            if pinned.get_untracked() {
                if let Some(el) = log_ref.get() {
                    el.set_scroll_top(el.scroll_height());
                }
            }
        });
        // Prepend scroll-restore — after an older page lands, shift
        // scrollTop by the height the DOM grew so the reader's position
        // holds steady.
        Effect::new(move |_| {
            let _ = older.read().len();
            let captured = prepend_height.get_value();
            if captured > 0.0 {
                prepend_height.set_value(0.0);
                if let Some(el) = log_ref.get() {
                    let grown = f64::from(el.scroll_height()) - captured;
                    if grown > 0.0 {
                        el.set_scroll_top(el.scroll_top() + grown as i32);
                    }
                }
            }
        });
    }

    let sending = RwSignal::new(false);
    let send_error: RwSignal<Option<String>> = RwSignal::new(None);
    let draft = RwSignal::new(String::new());
    // Held by another client → send asks to take over first.
    let confirm_takeover = RwSignal::new(false);

    // "Held" = `locked` while we are not the live attach. `locked` is
    // the store's union flag (any holder counts); `live` is our side —
    // `live_override` wins over the summary's feed-patched value.
    let is_held = move || {
        summary
            .get()
            .and_then(Result::ok)
            .is_some_and(|s| s.locked && !live_override.get().unwrap_or(s.live))
    };

    // POST /prompt — the actual send. Shared by `submit` and the
    // takeover-confirm path.
    let do_send = move |text: String| {
        sending.set(true);
        send_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match send_prompt(id, agent, text.clone()).await {
                Ok(()) => {
                    draft.set(String::new());
                    live.update(|t| t.push_user(&text));
                    client.invalidate_query(crate::api::pending_scope, ());
                    client.invalidate_query(crate::api::sessions_scope, ());
                }
                Err(e) => send_error.set(Some(e.to_string())),
            }
            sending.set(false);
        });
    };

    // The composer stays editable while the session is held
    // (draft-while-held is intended); send then detours through the
    // takeover `ConfirmDialog` instead of dying on a raw error.
    let submit = move || {
        let text = draft.get().trim().to_string();
        if text.is_empty() || sending.get() {
            return;
        }
        if is_held() {
            confirm_takeover.set(true);
            return;
        }
        do_send(text);
    };

    // "Take over and send" — attach with `takeover`, then send the
    // unchanged draft once the lock is ours.
    let takeover_send = move || {
        let text = draft.get().trim().to_string();
        if text.is_empty() || sending.get() {
            return;
        }
        sending.set(true);
        send_error.set(None);
        let id = session_id();
        let agent = agent();
        leptos::task::spawn_local(async move {
            match attach_session(id.clone(), agent.clone(), true).await {
                Ok(res) => {
                    live_override.set(Some(res.attached));
                    client.invalidate_query(crate::api::sessions_scope, ());
                    summary.refetch();
                    if res.attached && !res.read_only {
                        match send_prompt(id, agent, text.clone()).await {
                            Ok(()) => {
                                draft.set(String::new());
                                live.update(|t| t.push_user(&text));
                                client.invalidate_query(crate::api::pending_scope, ());
                            }
                            Err(e) => send_error.set(Some(e.to_string())),
                        }
                    } else {
                        send_error.set(Some(
                            "takeover refused — the session is still held".to_string(),
                        ));
                    }
                }
                Err(e) => send_error.set(Some(e.to_string())),
            }
            sending.set(false);
        });
    };

    let running = move || live.read().running;

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
                                view! {
                                    <ErrorBanner
                                        message=e.to_string()
                                        on_retry=Box::new(move || {
                                            summary.refetch();
                                            history.refetch();
                                        })
                                    />
                                }
                                .into_any()
                            }
                            (Ok(session), Ok(page)) => {
                                let locked = session.locked;
                                let busy = session.busy;
                                let live_flag = session.live;
                                let title_for_rename = session.title.clone();
                                // Cloned up front — the macro's move
                                // closures can't reach `session` fields
                                // once the tree above has captured them.
                                let prompt_cwd = session.cwd.clone();
                                let prompt_model = session.model.clone();
                                // Run-provenance spans → marker rows
                                // between transcript segments.
                                let spans: Vec<RunSpan> =
                                    session.spans.iter().map(Into::into).collect();
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
                                                    <Icon name="details"/>
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
                                                view! { <ErrorBanner message=e/> }
                                            })}
                                        </CardContent>
                                    </Card>
                                    {view! {
                                    <ConfirmDialog
                                        open=confirm_delete
                                        title="Delete this session?"
                                        body="This permanently deletes the session and its history from the store."
                                        confirm_label="Delete"
                                        destructive=true
                                        on_confirm=move || delete_confirmed.set(true)
                                    />
                                    }.into_any()}
                                    {view! {
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
                                    }.into_any()}
                                    {view! {
                                    <div class="relative flex min-h-40 flex-1 flex-col">
                                    <div
                                        class="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto rounded-lg border bg-card p-3"
                                        node_ref=log_ref
                                    >
                                        <OlderButton
                                            loading=older_loading
                                            load=load_older
                                            has_older=has_older
                                        />
                                        // All loaded history (earlier
                                        // pages + the first page) folds
                                        // through `build_rows` — system
                                        // blobs → one context card,
                                        // identical rows fold, spans/
                                        // model changes → markers.
                                        {move || {
                                            let mut msgs: Vec<HistoryMessageDto> = older
                                                .read()
                                                .iter()
                                                .flat_map(|p| p.messages.clone())
                                                .collect();
                                            msgs.extend(page.messages.iter().cloned());
                                            let inputs: Vec<RowInput> = msgs
                                                .iter()
                                                .map(|m| RowInput {
                                                    role: &m.role,
                                                    content: &m.content,
                                                    created_at: m.created_at,
                                                    model: m.model.as_deref(),
                                                    blocks: m.blocks.as_deref(),
                                                })
                                                .collect();
                                            build_rows(&inputs, &spans)
                                                .into_iter()
                                                .map(|row| match row {
                                                    ChatRow::Context(context) => view! {
                                                        <ContextCard context=context/>
                                                    }
                                                    .into_any(),
                                                    ChatRow::History { index, dup } => view! {
                                                        <HistoryRow
                                                            message=msgs[index].clone()
                                                            dup=dup
                                                        />
                                                    }
                                                    .into_any(),
                                                    ChatRow::Marker { label, .. } => view! {
                                                        <RunMarker label=label/>
                                                    }
                                                    .into_any(),
                                                })
                                                .collect::<Vec<_>>()
                                        }}
                                        <LiveLog live=live/>
                                    </div>
                                    // Live-stream connection pill —
                                    // the same floating-overlay pattern
                                    // as "Jump to bottom" below. Always
                                    // rendered; visibility is
                                    // class-only. Shows whenever the
                                    // stream exists but isn't `Open`
                                    // (connecting, or closed with the
                                    // infinite backoff retrying).
                                    <div
                                        data-name="ReconnectingPill"
                                        class=move || {
                                            let reconnecting = conn_state
                                                .get()
                                                .is_some_and(|s| {
                                                    s.get() != ConnectionReadyState::Open
                                                });
                                            let base = "absolute right-3 top-2 flex items-center gap-1.5 rounded-full border bg-popover px-3 py-1 text-xs text-muted-foreground shadow-md transition-opacity";
                                            if reconnecting {
                                                format!("{base} opacity-100")
                                            } else {
                                                format!("{base} opacity-0 pointer-events-none")
                                            }
                                        }
                                    >
                                        <span class="size-1.5 animate-pulse rounded-full bg-warning"></span>
                                        "Reconnecting…"
                                    </div>
                                    // Always rendered (SSR/hydrate
                                    // agree); visibility is class-only.
                                    <button
                                        type="button"
                                        class=move || {
                                            let base = "absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full border bg-popover px-3 py-1 text-xs shadow-md transition-opacity hover:bg-accent";
                                            if pinned.get() {
                                                format!("{base} opacity-0 pointer-events-none")
                                            } else {
                                                format!("{base} opacity-100")
                                            }
                                        }
                                        on:click=move |_| {
                                            pinned.set(true);
                                            if let Some(el) = log_ref.get() {
                                                el.set_scroll_top(el.scroll_height());
                                            }
                                        }
                                    >
                                        "↓ Jump to bottom"
                                    </button>
                                    </div>
                                    }.into_any()}
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
                                        cwd=prompt_cwd.clone()
                                        model=prompt_model.clone()
                                        queued=queued
                                        failed=failed
                                        submit=submit
                                    />
                                }
                                .into_any()
                            }
                        }
                    })
                }}
            </Suspense>
            <ConfirmDialog
                open=confirm_takeover
                title="Session is held by another client"
                body="Take over and send? The holding client is detached and the draft is sent from here."
                confirm_label="Take over and send"
                on_confirm=takeover_send
            />
        </section>
    }
}

/// "Load earlier messages" — fetches the page ending at the current
/// window's `start` index. The loader lives in `SessionPanel` so the
/// scroll-top trigger shares the same `loading` guard.
#[component]
fn OlderButton(
    loading: RwSignal<bool>,
    load: impl Fn() + 'static + Send + Sync + Copy,
    has_older: impl Fn() -> bool + 'static + Send + Sync + Copy,
) -> impl IntoView {
    view! {
        <Show when=move || has_older() fallback=|| ()>
            <div class="flex justify-center">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    disabled=loading
                    on_click=Box::new(load)
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
fn HistoryRow(message: HistoryMessageDto, dup: usize) -> impl IntoView {
    let role = message.role.clone();
    let text = message.text();
    if role == "tool" {
        let name = message.tool_name.clone().unwrap_or_else(|| "tool".into());
        let status = message.tool_status.clone().unwrap_or_else(|| "done".into());
        let error = status == "error" || status == "failed";
        let display = tool_summary(&name, message.args.as_deref(), &text, message.exit_code);
        let diffs: Vec<FileDiffView> = message
            .diffs
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(file_diff_view)
            .collect();
        let locations: Vec<String> = message
            .locations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|l| match l.line {
                Some(line) => format!("{}:{line}", l.path),
                None => l.path.clone(),
            })
            .collect();
        view! {
            // `open` keeps SSR/hydrate markup identical — collapsing
            // stays a client-side, native `<details>` toggle.
            <ToolBlock
                display=display
                status=status
                open=true
                error=error
                diffs=diffs
                locations=locations
                duration_ms=message.duration_ms
            />
        }
        .into_any()
    } else {
        let thinking = message.thinking.clone();
        let attachments = message
            .blocks
            .as_deref()
            .map(attachment_views)
            .unwrap_or_default();
        let usage = message.usage.clone();
        let class = match role.as_str() {
            "user" => "rounded-md border border-info/30 bg-info/5 px-3 py-2",
            _ => "rounded-md px-1 py-2",
        };
        view! {
            <article class=class>
                <header class="mb-1 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                    {role.clone()}
                    {message
                        .model
                        .clone()
                        .map(|m| view! {
                            <span class="font-mono normal-case text-muted-foreground/70">{m}</span>
                        })}
                    {(dup > 0).then(|| view! {
                        <span
                            data-name="DupFold"
                            class="rounded bg-muted px-1 py-px font-mono normal-case text-muted-foreground"
                            title=format!("{} identical messages folded", dup + 1)
                        >
                            {format!("×{}", dup + 1)}
                        </span>
                    })}
                </header>
                {thinking
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        view! {
                            <details class="mb-1 rounded border border-border/60 bg-muted/30" open>
                                <summary class="cursor-pointer select-none px-2 py-1 font-mono text-xs italic text-muted-foreground">
                                    "thinking"
                                </summary>
                                <pre class=TOOL_PRE>{t}</pre>
                            </details>
                        }
                    })}
                <Markdown text=text/>
                {attachments}
                {usage.map(|u| {
                    view! {
                        <p
                            data-name="UsageFooter"
                            class="mt-1 whitespace-pre text-[11px] text-muted-foreground/80"
                            title=format_usage(&u)
                        >
                            {usage_label(&u)}
                        </p>
                    }
                })}
            </article>
        }
        .into_any()
    }
}

/// All `system` messages rolled into one card — workspaces/platform in
/// the trigger line, Reports/Prompt/Rules/Skills behind tab buttons
/// (the visible set depends on what the store actually recorded).
#[component]
fn ContextCard(context: SystemContext) -> impl IntoView {
    let summary = [
        context.workspaces.first().cloned(),
        context.platform.clone(),
        context.os_version.clone(),
        context.date.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let summary = if summary.is_empty() {
        if context.rules.len() == 1 {
            "1 rule".to_string()
        } else if !context.rules.is_empty() {
            format!("{} rules", context.rules.len())
        } else {
            "System prompt".to_string()
        }
    } else {
        summary
    };
    // Tab order matches the old app: Reports / Prompt / Rules / Skills.
    let has_prompt = !context.prompt_text.is_empty() || !context.prompt_sections.is_empty();
    let tabs: Vec<(&str, bool, usize)> = vec![
        (
            "Reports",
            !context.reports.is_empty(),
            context.reports.len(),
        ),
        ("Prompt", has_prompt, 0),
        ("Rules", !context.rules.is_empty(), context.rules.len()),
        ("Skills", !context.skills.is_empty(), context.skills.len()),
    ];
    let visible: Vec<(usize, &str, usize)> = tabs
        .iter()
        .enumerate()
        .filter(|(_, (_, shown, _))| *shown)
        .map(|(i, (label, _, count))| (i, *label, *count))
        .collect();
    // `i` indexes the full tab list (Reports=0..Skills=3) — default to
    // the first *visible* tab, not index 0.
    let active = RwSignal::new(visible.first().map_or(0, |v| v.0));
    let tab_cls = move |on: bool| {
        if on {
            "shrink-0 rounded px-2.5 py-1 text-xs bg-secondary text-foreground"
        } else {
            "shrink-0 rounded px-2.5 py-1 text-xs text-muted-foreground hover:bg-accent/40 hover:text-foreground"
        }
    };
    view! {
        <details data-name="ContextCard" class="rounded-lg border border-border/60 bg-muted/30">
            <summary class="group flex cursor-pointer select-none items-center gap-2 px-3 py-2 text-xs text-muted-foreground transition-colors hover:text-foreground [&::-webkit-details-marker]:hidden">
                <span class="font-medium text-foreground/80">"Session context"</span>
                <span class="min-w-0 flex-1 truncate">{summary}</span>
                <svg
                    class="size-3.5 shrink-0 transition-transform group-open:rotate-180"
                    viewBox="0 0 12 12"
                    fill="none"
                >
                    <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </summary>
            <div class="border-t border-border/60 px-3 py-2.5">
                <div class="flex gap-0.5 overflow-x-auto">
                    {visible
                        .iter()
                        .map(|(i, label, count)| {
                            let i = *i;
                            let label = *label;
                            let count = *count;
                            view! {
                                <button
                                    type="button"
                                    class=move || tab_cls(active.get() == i).to_string()
                                    on:click=move |_| active.set(i)
                                >
                                    {label}
                                    {(count > 0).then(|| view! {
                                        <span class="ml-1 text-muted-foreground/70">{count}</span>
                                    })}
                                </button>
                            }
                        })
                        .collect::<Vec<_>>()}
                </div>
                <div class="mt-2 flex flex-col gap-1.5">
                    {move || match active.get() {
                        0 => context
                            .reports
                            .iter()
                            .map(|r| {
                                view! {
                                    <div class="rounded-md border border-border/60 bg-muted/30">
                                        <div class="flex min-w-0 items-center gap-2 px-2.5 pt-1.5 text-[11px]">
                                            {r.title.clone().map(|t| {
                                                let label = t.clone();
                                                view! {
                                                    <span class="min-w-0 flex-1 truncate font-medium text-foreground/70" title=t>
                                                        {label}
                                                    </span>
                                                }
                                            })}
                                            {r.agent_id.clone().map(|id| view! {
                                                <code class="shrink-0 text-muted-foreground/80">
                                                    {format!("agent {id}")}
                                                </code>
                                            })}
                                        </div>
                                        <pre class="max-h-64 overflow-auto whitespace-pre-wrap p-2 text-xs text-muted-foreground">
                                            {r.body.clone()}
                                        </pre>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any(),
                        1 => {
                            let mut items: Vec<AnyView> = context
                                .prompt_sections
                                .iter()
                                .map(|s| {
                                    view! {
                                        <details class="rounded-md border border-border/60 bg-muted/30">
                                            <summary class="cursor-pointer select-none px-2.5 py-1.5 text-xs font-medium text-foreground/80">
                                                {s.title.clone()}
                                            </summary>
                                            <pre class="max-h-64 overflow-auto whitespace-pre-wrap border-t border-border/60 px-2.5 py-2 text-xs text-muted-foreground">
                                                {s.body.clone()}
                                            </pre>
                                        </details>
                                    }
                                    .into_any()
                                })
                                .collect();
                            if !context.prompt_text.is_empty() {
                                items.push(if context.prompt_sections.is_empty() {
                                    view! {
                                        <pre class="max-h-80 overflow-auto whitespace-pre-wrap rounded-md bg-muted/40 p-2.5 text-xs text-muted-foreground">
                                            {context.prompt_text.clone()}
                                        </pre>
                                    }
                                    .into_any()
                                } else {
                                    view! {
                                        <details class="rounded-md border border-border/60 bg-muted/30">
                                            <summary class="cursor-pointer select-none px-2.5 py-1.5 text-xs font-medium text-foreground/80">
                                                "Full prompt"
                                            </summary>
                                            <pre class="max-h-64 overflow-auto whitespace-pre-wrap border-t border-border/60 px-2.5 py-2 text-xs text-muted-foreground">
                                                {context.prompt_text.clone()}
                                            </pre>
                                        </details>
                                    }
                                    .into_any()
                                });
                            }
                            items.into_any()
                        }
                        2 => context
                            .rules
                            .iter()
                            .map(|rule| {
                                if let Some(content) = &rule.content {
                                    view! {
                                        <details class="rounded-md border border-border/60 bg-muted/30">
                                            <summary class="flex cursor-pointer select-none items-center gap-2 px-2.5 py-1.5 text-xs">
                                                <span class="min-w-0 flex-1 truncate font-medium text-foreground/80">
                                                    {rule.name.clone()}
                                                </span>
                                                <code class="max-w-2/5 shrink-0 truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground" title=rule.path.clone()>
                                                    {rule.path.clone()}
                                                </code>
                                            </summary>
                                            <pre class="max-h-64 overflow-auto whitespace-pre-wrap border-t border-border/60 px-2.5 py-2 text-xs text-muted-foreground">
                                                {content.clone()}
                                            </pre>
                                        </details>
                                    }
                                    .into_any()
                                } else {
                                    view! {
                                        <div class="flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-2.5 py-1.5 text-xs">
                                            <span class="min-w-0 flex-1 truncate font-medium text-foreground/80">
                                                {rule.name.clone()}
                                            </span>
                                            <code class="max-w-2/5 shrink-0 truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground" title=rule.path.clone()>
                                                {rule.path.clone()}
                                            </code>
                                        </div>
                                    }
                                    .into_any()
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any(),
                        _ => context
                            .skills
                            .iter()
                            .map(|skill| {
                                view! {
                                    <div class="flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-2.5 py-1.5 text-xs">
                                        <span class="min-w-0 flex-1 truncate font-medium text-foreground/80" title=skill.name.clone()>
                                            {skill.name.clone()}
                                        </span>
                                        {skill.description.clone().map(|d| {
                                            let text = d.clone();
                                            view! {
                                                <span class="shrink-0 truncate text-muted-foreground" title=d>{text}</span>
                                            }
                                        })}
                                        {skill.source.clone().map(|s| {
                                            let label = s.clone();
                                            view! {
                                                <code class="max-w-2/5 shrink-0 truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground" title=s>
                                                    {label}
                                                </code>
                                            }
                                        })}
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any(),
                    }}
                </div>
            </div>
        </details>
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
                        match e.kind {
                            LiveKind::Assistant => {
                                let surface = if e.error {
                                    "border-destructive/50 bg-destructive/10"
                                } else {
                                    "border-info/30 bg-info/5"
                                };
                                view! {
                                    <article class=format!("rounded-md border px-3 py-2 {surface}")>
                                        <header class="mb-1 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                                            {(!e.done).then(|| view! {
                                                <span class="size-1.5 animate-pulse rounded-full bg-info"></span>
                                            })}
                                            {e.title.clone()}
                                            {e.error.then(|| view! {
                                                <Badge variant=BadgeVariant::Destructive>"error"</Badge>
                                            })}
                                        </header>
                                        <Markdown text=e.text.clone()/>
                                    </article>
                                }
                                .into_any()
                            }
                            LiveKind::Reasoning => {
                                let class = if e.error {
                                    "rounded-md border border-destructive/50 bg-destructive/10"
                                } else {
                                    "rounded-md border border-border/60 bg-muted/40"
                                };
                                view! {
                                    // Open while streaming; settles
                                    // closed once the thinking ends.
                                    <details class=class open=!e.done>
                                        <summary class="flex cursor-pointer select-none items-center gap-2 px-3 py-2 font-mono text-xs italic text-muted-foreground">
                                            {(!e.done).then(|| view! {
                                                <span class="size-1.5 shrink-0 animate-pulse rounded-full bg-info"></span>
                                            })}
                                            "thinking"
                                        </summary>
                                        <pre class=TOOL_PRE>{e.text.clone()}</pre>
                                    </details>
                                }
                                .into_any()
                            }
                            LiveKind::Marker => {
                                view! { <RunMarker label=e.title.clone()/> }.into_any()
                            }
                            LiveKind::Tool => {
                                let status = if !e.done {
                                    "running…"
                                } else if e.error {
                                    "error"
                                } else {
                                    "done"
                                };
                                // `text` accumulates the args deltas;
                                // `result` lands on ToolCallResult.
                                let args = (!e.text.trim().is_empty()).then(|| e.text.clone());
                                let result = e.result.clone().unwrap_or_default();
                                let display = tool_summary(
                                    &e.title,
                                    args.as_deref(),
                                    &result,
                                    None,
                                );
                                let diffs: Vec<FileDiffView> =
                                    e.diffs.iter().map(file_diff_view).collect();
                                let locations: Vec<String> = e
                                    .locations
                                    .iter()
                                    .map(|l| match l.line {
                                        Some(line) => format!("{}:{line}", l.path),
                                        None => l.path.clone(),
                                    })
                                    .collect();
                                let contents = tool_content_segments(&e.contents);
                                view! {
                                    <ToolBlock
                                        display=display
                                        status=status
                                        open=!e.done
                                        error=e.error
                                        diffs=diffs
                                        locations=locations
                                        contents=contents
                                    />
                                }
                                .into_any()
                            }
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        </div>
    }
}

/// The prompt composer — multiline textarea (Enter sends, Shift+Enter
/// adds a newline) that auto-grows to ~10 lines in the browser, plus a
/// footer strip with the session's `cwd`/model as context chips and the
/// send button. `cwd` is display-only: the node's `/api/fs` listing
/// exists but no `NodeApi` port method reaches it, so there's no picker
/// to drive. `model` is likewise display-only — `send_prompt` takes no
/// model argument. `queued`/`failed` are this session's outbox counts,
/// surfaced as a one-line note while writes wait on a down node.
#[component]
fn PromptBox(
    draft: RwSignal<String>,
    sending: RwSignal<bool>,
    send_error: RwSignal<Option<String>>,
    cwd: String,
    model: Option<String>,
    queued: usize,
    failed: usize,
    submit: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    // Enter sends; Shift+Enter inserts a newline.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            submit();
        }
    };
    let area_ref = NodeRef::<leptos::html::Textarea>::new();
    // Auto-grow, browser only: refit the height to the content on every
    // draft change (keystrokes and the clear-after-send reset), capped
    // at ~10 `text-sm` lines. SSR keeps the `rows=3` markup untouched —
    // effects never run there.
    #[cfg(feature = "hydrate")]
    {
        /// 10 lines at `text-sm`'s 20px line-height + `py-2` padding.
        const MAX_HEIGHT_PX: i32 = 216;
        Effect::new(move |_| {
            let _ = draft.read();
            if let Some(el) = area_ref.get() {
                // `el` is a tachys `HtmlElement` — `style()` would
                // resolve to its builder trait; go through web_sys.
                let style = web_sys::HtmlElement::style(&el);
                let _ = style.set_property("height", "auto");
                let full = el.scroll_height();
                let _ = style.set_property("height", &format!("{}px", full.min(MAX_HEIGHT_PX)));
                let _ = style.set_property(
                    "overflow-y",
                    if full > MAX_HEIGHT_PX {
                        "auto"
                    } else {
                        "hidden"
                    },
                );
            }
        });
    }
    view! {
        <div class="space-y-2">
            {move || send_error.get().map(|e| view! { <ErrorBanner message=e/> })}
            {(queued > 0).then(|| {
                view! {
                    <p class="flex items-center gap-1.5 text-xs text-muted-foreground">
                        <span class="size-1.5 shrink-0 rounded-full bg-warning"></span>
                        {format!(
                            "{queued} {} queued — will send when the node is up",
                            if queued == 1 { "write" } else { "writes" },
                        )}
                    </p>
                }
            })}
            {(failed > 0).then(|| {
                view! {
                    <p class="text-xs text-destructive">
                        {format!(
                            "{failed} {} failed to reach the node",
                            if failed == 1 { "write" } else { "writes" },
                        )}
                    </p>
                }
            })}
            <textarea
                class=TEXTAREA_CLASS
                placeholder="Message the agent…  (Enter to send, Shift+Enter for newline)"
                prop:value=move || draft.get()
                on:input=move |ev| draft.set(event_target_value(&ev))
                on:keydown=on_keydown
                rows=3
                node_ref=area_ref
            ></textarea>
            <div class="flex items-center gap-2">
                {(!cwd.is_empty()).then(|| cwd.clone()).map(|c| {
                    let tip = c.clone();
                    view! {
                        <code
                            class="min-w-0 max-w-52 shrink truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground"
                            title=tip
                        >
                            {c}
                        </code>
                    }
                })}
                {model.map(|m| {
                    view! {
                        <span
                            class="shrink-0 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground"
                            title="model"
                        >
                            {m}
                        </span>
                    }
                })}
                <span class="ml-auto hidden shrink-0 text-[11px] text-muted-foreground sm:inline">
                    "Shift+Enter for newline"
                </span>
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
                {move || error.get().map(|e| view! { <ErrorBanner message=e/> })}
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
                    Some(Err(e)) => {
                        view! {
                            <ErrorBanner
                                message=e.to_string()
                                on_retry=Box::new(move || checkpoints.refetch())
                            />
                        }
                        .into_any()
                    }
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
            {move || error.get().map(|e| view! { <ErrorBanner message=e/> })}
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
