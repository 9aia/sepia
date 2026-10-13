//! `/sessions/:id` — history (paged via `?before`), live SSE stream
//! folded into a transcript, and the prompt box.

use leptos::prelude::*;
use leptos_meta::Title;
#[cfg(not(feature = "hydrate"))]
use leptos_router::NavigateOptions;
use leptos_router::components::A;
#[cfg(feature = "hydrate")]
use leptos_router::hooks::use_navigate;
use leptos_router::hooks::{use_params_map, use_query_map};
use leptos_use::core::ConnectionReadyState;
use sepia_web_core::composer::{
    Attachment, attachments_payload, byte_label, convert_targets, draft_with_quote, export_href,
};
#[cfg(feature = "hydrate")]
use sepia_web_core::composer::{MAX_ATTACHMENT_CHARS, MAX_ATTACHMENTS, attachment_chars};
use sepia_web_core::filter::session_href;
use sepia_web_core::history::{
    ChatRow, FileDiffView, RowInput, RunSpan, SystemContext, ToolSegment, build_rows,
    file_diff_view, format_duration, format_usage, tool_content_segments, tool_summary,
    usage_label,
};
use sepia_web_core::rewind::{RewindSelector, rewind_node_id, rewind_result_summary};
use sepia_web_core::stream::ReconnectTracker;
use sepia_web_core::transcript::{LiveKind, LiveTranscript, PendingPermission};
use serde_json::Value;

use crate::api::{
    answer_permission, archive_session, attach_session, cancel_run, convert_session,
    delete_session, detach_session, pin_session, rename_session, restore_checkpoint,
    rewind_session, send_prompt, session_history, set_session_model, set_session_projects,
};
use crate::components::icons::Icon;
use crate::components::toast::use_toast;
use crate::components::{
    Badge, BadgeVariant, Button, ButtonClass, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, Dropdown, ErrorBanner, Input, MenuItem,
    MenuSeparator, Sheet, SheetBody, SheetHeader, SheetTitle, Skeleton,
};
use crate::dto::{CheckpointDto, HistoryMessageDto, HistoryPageDto};
use crate::markdown::Markdown;
use crate::pages::SessionListPanel;
use tw_merge::IntoTailwindClass;

const PAGE_SIZE: i64 = 100;

/// Shared `<pre>` body for tool output and thinking dumps.
const TOOL_PRE: &str = "max-h-80 overflow-auto whitespace-pre-wrap break-words border-t \
                        border-border/60 px-3 py-2 font-mono text-xs text-muted-foreground";

/// `/sessions/:id` — standalone deep link: the same master-detail
/// pair `/` composes — the rail's Sessions panel (the session list)
/// beside `SessionPanel`, so a shared link lands inside the shell
/// with the list still open. The `:id` param is the panel's selected
/// row; `?agent=` picks the store like on `/`.
#[component]
pub fn SessionDetailPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let session_id = move || params.read().get("id").unwrap_or_default();
    let agent = move || non_empty_str(&query.read().get("agent").unwrap_or_default());
    let selected_id: Signal<Option<String>> = Signal::derive(move || non_empty_str(&session_id()));
    view! {
        <div class="flex h-full min-h-0 w-full">
            <SessionListPanel selected_id=selected_id/>
            <div class="flex min-w-0 flex-1 flex-col">
                {move || {
                    view! {
                        <SessionPanel session_id=session_id() agent=agent()/>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}

fn non_empty_str(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_string())
}

/// Optimistic echo for a sent prompt — an attachments-only send still
/// needs a transcript line (the prompt API doesn't stream user
/// messages back).
fn prompt_echo(text: &str, attachment_count: usize) -> String {
    if !text.is_empty() {
        return text.to_string();
    }
    format!(
        "[{attachment_count} attachment{}]",
        if attachment_count == 1 { "" } else { "s" }
    )
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
    // `cv-auto` virtualizes the row (content-visibility) — see
    // input.css; every direct child of the transcript scroller wears it.
    let class = if error {
        "cv-auto rounded-md border border-destructive/40 bg-destructive/10"
    } else {
        "cv-auto rounded-md border border-border bg-muted/40"
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
        <div data-name="RunMarker" class="cv-auto flex items-center gap-2 px-1 py-0.5">
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
    let owner = StoredValue::new(Owner::current().unwrap_or_default());
    // Shadow the signals with the closure shape the body was written
    // against — `session_id()`/`agent()` return owned values.
    let session_id = move || session_id.get();
    let agent = move || agent.get();
    // `StoredValue` keeps the navigate fn Copy-able into handlers.
    // `new_local` stores a SendWrapper in the owner arena — on SSR the
    // owner cleans up on an arbitrary tokio worker and the guard
    // panics, aborting the response stream. SSR never navigates.
    #[cfg(feature = "hydrate")]
    let _navigate = StoredValue::new_local(use_navigate());
    #[cfg(not(feature = "hydrate"))]
    let _navigate = StoredValue::new(|_: &str, _: NavigateOptions| {});

    let client = crate::api::query_client();
    let summary = client.resource(crate::api::session_scope, move || (session_id(), agent()));
    let history = client.resource(crate::api::history_scope, move || (session_id(), agent()));
    // Hub outbox rows — filtered to this session for the queued/failed
    // badges. Not session-scoped server-side; the list is small.
    let pending = crate::api::shared().pending;
    // The projects catalog — the details sheet's membership toggles.
    let projects = crate::api::shared().projects;

    let toast = use_toast();
    // Action state lives at page level: a summary refetch re-runs the
    // Suspend subtree, and signals owned inside it would reset.
    let acting = RwSignal::new(false);
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
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            if let Some(res) = toast.outcome(attach_session(id, agent, takeover).await, "Attached")
            {
                live_override.set(Some(res.attached));
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
        });
    };
    let do_detach = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(detach_session(id, agent).await, "Detached")
                .is_some()
            {
                live_override.set(Some(false));
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
        });
    };
    let do_cancel = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            let _ = toast.outcome(cancel_run(id, agent).await, "Cancel requested");
            acting.set(false);
        });
    };
    let do_rename = move || {
        let title = rename_draft.get().trim().to_string();
        if title.is_empty() || acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(rename_session(id, agent, title).await, "Renamed")
                .is_some()
            {
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
        });
    };
    let do_delete = move || {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(delete_session(id, agent).await, "Deleted")
                .is_some()
            {
                client.invalidate_query(crate::api::sessions_scope, ());
                crate::app::nav_external("/");
            } else {
                acting.set(false);
            }
        });
    };
    // `POST /convert` — copy the session into another agent's store.
    // The response carries the *new* session's id; jump straight to it
    // on the merged list.
    let do_convert = move |target: &'static str| {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            match convert_session(id, agent, target.to_string()).await {
                Ok(new_id) => {
                    toast.success(format!("Converted to {target}"));
                    client.invalidate_query(crate::api::sessions_scope, ());
                    crate::app::nav_external(&session_href(&new_id, target));
                }
                Err(e) => toast.error(e.to_string()),
            }
            acting.set(false);
        });
    };
    // Meta-overlay toggles — `PATCH /api/sessions/{id}` `{pinned}` /
    // `{archived}` / `{projectIds}`, same write path as the list's
    // context menu.
    let do_pin = move |pinned: bool| {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        let ok = if pinned { "Pinned" } else { "Unpinned" };
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(pin_session(id, agent, pinned).await, ok)
                .is_some()
            {
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
        });
    };
    let do_archive = move |archived: bool| {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        let ok = if archived { "Archived" } else { "Unarchived" };
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(archive_session(id, agent, archived).await, ok)
                .is_some()
            {
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
        });
    };
    // `{projectIds}` replaces wholesale — read the freshest summary
    // for the current set so toggling one project keeps the rest.
    let toggle_project = move |key: String| {
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        let ids = summary.get().and_then(Result::ok).map_or_else(
            || vec![key.clone()],
            |s| {
                if s.project_ids.contains(&key) {
                    s.project_ids
                        .iter()
                        .filter(|p| **p != key)
                        .cloned()
                        .collect()
                } else {
                    let mut ids = s.project_ids.clone();
                    ids.push(key.clone());
                    ids
                }
            },
        );
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(
                    set_session_projects(id, agent, ids).await,
                    "Projects updated",
                )
                .is_some()
            {
                client.invalidate_query(crate::api::sessions_scope, ());
                summary.refetch();
            }
            acting.set(false);
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
    // The tracker gates the pill to *post-open* outages — the initial
    // connect stays quiet.
    let conn_tracker = RwSignal::new(ReconnectTracker::default());

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
        crate::app::spawn_under(owner.get_value(), async move {
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
                (Some(agent_name), Some(true)) if slot.with_value(Option::is_none) => {
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
                    // A fresh stream gets a fresh tracker — its own
                    // initial connect is quiet too.
                    conn_tracker.write().reset();
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
    let draft = RwSignal::new(String::new());
    // Staged prompt attachments — chips above the textarea; staged by
    // the paste/file-picker handlers and cleared on send.
    let attachments: RwSignal<Vec<Attachment>> = RwSignal::new(Vec::new());
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
    // Our side of the attach — `true` while this client holds the live
    // attach (the composer sends straight through).
    let is_live = move || {
        summary
            .get()
            .and_then(Result::ok)
            .is_some_and(|s| live_override.get().unwrap_or(s.live))
    };

    // POST /prompt — the actual send. Shared by `submit` and the
    // takeover-confirm path.
    let do_send = move |text: String| {
        sending.set(true);
        let parts = attachments_payload(&attachments.get_untracked());
        let echo = prompt_echo(&text, parts.len());
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            match send_prompt(id, agent, text, parts).await {
                Ok(()) => {
                    draft.set(String::new());
                    attachments.set(Vec::new());
                    live.update(|t| t.push_user(&echo));
                    client.invalidate_query(crate::api::pending_scope, ());
                    client.invalidate_query(crate::api::sessions_scope, ());
                }
                Err(e) => toast.error(e.to_string()),
            }
            sending.set(false);
        });
    };

    // Per-message rewind — a transcript row's `nodeId` is the cut
    // point; the page-level `ConfirmDialog` guards the destructive
    // `POST /rewind`. `rewind_target` holds the row being asked about
    // while the dialog is open.
    let rewind_target: RwSignal<Option<i64>> = RwSignal::new(None);
    let confirm_rewind = RwSignal::new(false);
    let on_rewind = move |node: i64| {
        rewind_target.set(Some(node));
        confirm_rewind.set(true);
    };
    let do_rewind = move || {
        let Some(node) = rewind_target.get() else {
            return;
        };
        rewind_target.set(None);
        if acting.get() {
            return;
        }
        acting.set(true);
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            match rewind_session(id, agent, RewindSelector::NodeId(node)).await {
                Ok(result) => {
                    toast.success(rewind_result_summary(result.removed));
                    // The plane detaches a live attach before it
                    // truncates — drop the local live view to match,
                    // and drop the prepended older pages (their tail
                    // window no longer exists).
                    live.set(LiveTranscript::default());
                    live_override.set(Some(false));
                    older.set(Vec::new());
                    history.refetch();
                    summary.refetch();
                    checkpoints.refetch();
                    client.invalidate_query(crate::api::sessions_scope, ());
                }
                Err(e) => toast.error(e.to_string()),
            }
            acting.set(false);
        });
    };
    // `ConfirmDialog::on_confirm` requires `Send` — the dialog flips
    // this flag and the effect runs the rewind inside the owner, the
    // same detour `delete_confirmed` uses.
    let rewind_confirmed = RwSignal::new(false);
    Effect::new(move |_| {
        if rewind_confirmed.get() {
            rewind_confirmed.set(false);
            do_rewind();
        }
    });

    // Send-through-attach — attach (`takeover` when the dialog asked),
    // then fire the unchanged draft once the lock is ours. A plain
    // attach that lands read-only means the session is held by
    // someone else: the takeover dialog takes it from there.
    let attach_and_send = move |takeover: bool| {
        let text = draft.get().trim().to_string();
        let empty = text.is_empty() && attachments.read().is_empty();
        if empty || sending.get() {
            return;
        }
        sending.set(true);
        let parts = attachments_payload(&attachments.get_untracked());
        let echo = prompt_echo(&text, parts.len());
        let id = session_id();
        let agent = agent();
        crate::app::spawn_under(owner.get_value(), async move {
            match attach_session(id.clone(), agent.clone(), takeover).await {
                Ok(res) => {
                    live_override.set(Some(res.attached));
                    client.invalidate_query(crate::api::sessions_scope, ());
                    summary.refetch();
                    if res.attached && !res.read_only {
                        match send_prompt(id, agent, text, parts).await {
                            Ok(()) => {
                                draft.set(String::new());
                                attachments.set(Vec::new());
                                live.update(|t| t.push_user(&echo));
                                client.invalidate_query(crate::api::pending_scope, ());
                            }
                            Err(e) => toast.error(e.to_string()),
                        }
                    } else {
                        confirm_takeover.set(true);
                    }
                }
                Err(e) => toast.error(e.to_string()),
            }
            sending.set(false);
        });
    };

    // The composer stays editable while the session is held
    // (draft-while-held is intended); send then detours through the
    // takeover `ConfirmDialog` instead of dying on a raw error.
    let submit = move || {
        let text = draft.get().trim().to_string();
        let empty = text.is_empty() && attachments.read().is_empty();
        if empty || sending.get() {
            return;
        }
        if is_held() {
            confirm_takeover.set(true);
            return;
        }
        // Not attached — attach first, then the draft lands (the old
        // app attached on open; attach-on-send keeps "type and it
        // goes" without spawning the agent on browse).
        if !is_live() {
            attach_and_send(false);
            return;
        }
        do_send(text);
    };

    // The composer textarea — lifted to the page so the transcript's
    // per-message "reply" action can focus it after inserting the
    // `> `-quoted block into the draft.
    let composer_ref = NodeRef::<leptos::html::Textarea>::new();
    let quote_reply = move |text: &str| {
        draft.update(|d| *d = draft_with_quote(d, text));
        if let Some(el) = composer_ref.get() {
            let _ = el.focus();
        }
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
                                // Per-message rewind affordance — the
                                // node's flag AND a writable, idle
                                // session (held/busy rows hide it;
                                // the node would refuse anyway).
                                let can_rewind = session.can_rewind;
                                let rewind_ok = Signal::derive(move || {
                                    can_rewind && !busy && !is_held() && !running()
                                });
                                let title_for_rename = session.title.clone();
                                // Cloned up front — the macro's move
                                // closures can't reach `session` fields
                                // once the tree above has captured them.
                                let prompt_model = session.model.clone();
                                // Convert/export surfaces — the store
                                // the session lives in is excluded
                                // from the convert targets.
                                let detail_id = session.id.clone();
                                let detail_agent = session.agent.clone();
                                // Header + sheet captures.
                                let head_title = if session.title.trim().is_empty() {
                                    "Untitled session".to_string()
                                } else {
                                    session.title.clone()
                                };
                                let head_cwd = session.cwd.clone();
                                let copy_id = session.id.clone();
                                let copy_cwd = session.cwd.clone();
                                // Own clones for the Advanced rows —
                                // the view! child closures move their
                                // captures, so `detail_*`/`copy_*`
                                // can't be shared across sections.
                                let adv_id = session.id.clone();
                                let adv_cwd = session.cwd.clone();
                                let adv_agent = session.agent.clone();
                                let pinned_flag = session.pinned;
                                let archived_flag = session.archived;
                                let sheet_node = session
                                    .node
                                    .clone()
                                    .unwrap_or_else(|| "local".to_string());
                                let session_node_opt = session.node.clone();
                                let sheet_updated = session.updated_at.clone();
                                let sheet_source = session.source.clone();
                                let sheet_agent_id = session.agent_id.clone();
                                let sheet_spans = session.spans.clone();
                                let sheet_lock_pid = session.lock_holder_pid;
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
                                    // The old app's ChatHeader — a
                                    // slim bar: the title block opens
                                    // the details sheet, status chips
                                    // and a ⋯ action menu sit right.
                                    <header
                                        data-name="ChatHeader"
                                        class="flex items-center gap-2"
                                    >
                                        // Mobile-only back affordance —
                                        // on lg+ the Sessions panel is
                                        // already beside the chat.
                                        <A
                                            href="/"
                                            attr:class="inline-flex size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground lg:hidden"
                                            attr:aria-label="Back to sessions"
                                            attr:title="Back to sessions"
                                        >
                                            <Icon name="arrow-left" class="size-4"/>
                                        </A>
                                        <button
                                            type="button"
                                            class="min-w-0 flex-1 cursor-pointer rounded-md px-1 py-0.5 text-left transition-colors hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring"
                                            aria-label=format!("Session details: {head_title}")
                                            title="Session details"
                                            on:click=move |_| details_open.set(true)
                                        >
                                            <h2 class="m-0 truncate text-[15px] font-medium">
                                                {head_title.clone()}
                                            </h2>
                                            <span
                                                class="block truncate text-xs text-muted-foreground"
                                                title=head_cwd.clone()
                                            >
                                                {head_cwd.clone()}
                                            </span>
                                        </button>
                                        <div class="flex shrink-0 items-center gap-2">
                                            {move || {
                                                (busy || running()).then(|| view! {
                                                    <Badge variant=BadgeVariant::Destructive>
                                                        "busy"
                                                    </Badge>
                                                })
                                            }}
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
                                            <Dropdown
                                                icon="ellipsis-vertical"
                                                aria_label="Session actions"
                                                menu_class="w-52"
                                            >
                                                {move || {
                                                    if live_override.get().unwrap_or(live_flag) {
                                                        view! {
                                                            <MenuItem
                                                                label="Detach"
                                                                icon="unplug"
                                                                on_click=Box::new(do_detach)
                                                            />
                                                        }
                                                            .into_any()
                                                    } else if locked {
                                                        view! {
                                                            <MenuItem
                                                                label="Attach (takeover)"
                                                                icon="plug"
                                                                on_click=Box::new(move || do_attach(true))
                                                            />
                                                        }
                                                            .into_any()
                                                    } else {
                                                        view! {
                                                            <MenuItem
                                                                label="Attach"
                                                                icon="plug"
                                                                on_click=Box::new(move || do_attach(false))
                                                            />
                                                        }
                                                            .into_any()
                                                    }
                                                }}
                                                {move || {
                                                    (busy || running()).then(|| {
                                                        view! {
                                                            <MenuItem
                                                                label="Cancel run"
                                                                icon="octagon-x"
                                                                on_click=Box::new(do_cancel)
                                                            />
                                                        }
                                                    })
                                                }}
                                                <MenuSeparator/>
                                                {move || {
                                                    let pinned = summary
                                                        .get()
                                                        .and_then(Result::ok)
                                                        .map_or(pinned_flag, |s| s.pinned);
                                                    view! {
                                                        <MenuItem
                                                            label=if pinned { "Unpin" } else { "Pin" }
                                                            icon="pin"
                                                            on_click=Box::new(move || do_pin(!pinned))
                                                        />
                                                    }
                                                }}
                                                {move || {
                                                    let archived = summary
                                                        .get()
                                                        .and_then(Result::ok)
                                                        .map_or(archived_flag, |s| s.archived);
                                                    view! {
                                                        <MenuItem
                                                            label=if archived { "Unarchive" } else { "Archive" }
                                                            icon="archive"
                                                            on_click=Box::new(move || do_archive(!archived))
                                                        />
                                                    }
                                                }}
                                                <MenuItem
                                                    label="Rename…"
                                                    icon="pencil"
                                                    on_click=Box::new(move || {
                                                        rename_draft.set(title_for_rename.clone());
                                                        details_open.set(true);
                                                    })
                                                />
                                                <MenuItem
                                                    label="Details"
                                                    icon="info"
                                                    on_click=Box::new(move || details_open.set(true))
                                                />
                                                <MenuSeparator/>
                                                <MenuItem
                                                    label="Copy session ID"
                                                    icon="copy"
                                                    on_click=Box::new(move || {
                                                        crate::components::copy_to_clipboard(&copy_id);
                                                    })
                                                />
                                                <MenuItem
                                                    label="Copy path"
                                                    icon="copy"
                                                    on_click=Box::new(move || {
                                                        crate::components::copy_to_clipboard(&copy_cwd);
                                                    })
                                                />
                                                <MenuSeparator/>
                                                <MenuItem
                                                    label="Delete session"
                                                    icon="trash"
                                                    destructive=true
                                                    on_click=Box::new(move || confirm_delete.set(true))
                                                />
                                            </Dropdown>
                                        </div>
                                    </header>
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
                                                <Icon name="x" class="size-4"/>
                                            </Button>
                                        </SheetHeader>
                                        <SheetBody class="space-y-5">
                                            // The old drawer's info
                                            // rows — status + owner at
                                            // a glance.
                                            <div class="divide-y divide-border/50">
                                                <DetailRow label="Last active">
                                                    {crate::time::relative(
                                                        &sheet_updated,
                                                        crate::time::now_ms(),
                                                    )}
                                                </DetailRow>
                                                <DetailRow label="Status">
                                                    {move || {
                                                        let held = is_held();
                                                        let live_now =
                                                            live_override.get().unwrap_or(live_flag);
                                                        if locked || held {
                                                            view! {
                                                                <span title=sheet_lock_pid
                                                                    .map_or_else(String::new, |p| {
                                                                        format!("Locked by pid {p:.0}")
                                                                    })
                                                                >
                                                                    <Badge variant=BadgeVariant::Destructive>
                                                                        "locked"
                                                                    </Badge>
                                                                </span>
                                                            }
                                                                .into_any()
                                                        } else if busy || running() {
                                                            view! {
                                                                <Badge variant=BadgeVariant::Destructive>
                                                                    "busy"
                                                                </Badge>
                                                            }
                                                                .into_any()
                                                        } else {
                                                            let text = if live_now { "live" } else { "free" };
                                                            view! {
                                                                <Badge variant=BadgeVariant::Secondary>
                                                                    {text}
                                                                </Badge>
                                                            }
                                                                .into_any()
                                                        }
                                                    }}
                                                </DetailRow>
                                                <DetailRow label="Server">{sheet_node.clone()}</DetailRow>
                                            </div>
                                            // The old drawer's
                                            // "Advanced" fold — ids,
                                            // paths, provenance.
                                            <details class="group/details">
                                                <summary class="flex w-full cursor-pointer select-none items-center justify-between rounded-md px-1 py-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground [&::-webkit-details-marker]:hidden">
                                                    "Advanced"
                                                    <Icon
                                                        name="chevron-down"
                                                        class="size-3.5 transition-transform group-open/details:rotate-180"
                                                    />
                                                </summary>
                                                <div class="divide-y divide-border/50">
                                                    <DetailRow
                                                        label="ID"
                                                        copy=adv_id.clone()
                                                        mono=true
                                                    >
                                                        {adv_id.clone()}
                                                    </DetailRow>
                                                    <DetailRow
                                                        label="Working directory"
                                                        copy=adv_cwd.clone()
                                                        mono=true
                                                    >
                                                        {adv_cwd.clone()}
                                                    </DetailRow>
                                                    <DetailRow label="Agent">
                                                        <Badge variant=BadgeVariant::Secondary>
                                                            {adv_agent.clone()}
                                                        </Badge>
                                                        {sheet_agent_id
                                                            .clone()
                                                            .filter(|a| !a.is_empty())
                                                            .map(|a| view! {
                                                                <span
                                                                    class="text-muted-foreground"
                                                                    title="Sub-agent identity within the parent's team"
                                                                >
                                                                    {format!("· {a}")}
                                                                </span>
                                                            })}
                                                    </DetailRow>
                                                    {(!sheet_source.is_empty()).then(|| {
                                                        view! {
                                                            <DetailRow label="Source">
                                                                {sheet_source.clone()}
                                                            </DetailRow>
                                                        }
                                                    })}
                                                    {(!sheet_spans.is_empty()).then(|| {
                                                        view! {
                                                            <div class="flex flex-col gap-0.5 py-2 text-sm">
                                                                <span class="text-xs text-muted-foreground">
                                                                    "Runs"
                                                                </span>
                                                                <div class="flex flex-col gap-1.5">
                                                                    {sheet_spans
                                                                        .iter()
                                                                        .map(|span| {
                                                                            view! {
                                                                                <span class="flex items-center gap-1.5 text-xs">
                                                                                    <Badge
                                                                                        variant=BadgeVariant::Secondary
                                                                                        class="font-normal"
                                                                                    >
                                                                                        {format!(
                                                                                            "{} @ {}",
                                                                                            span.agent,
                                                                                            if span.node.is_empty() { "local" } else { &span.node },
                                                                                        )}
                                                                                    </Badge>
                                                                                    <span class="text-muted-foreground">
                                                                                        {crate::time::relative_ms(
                                                                                            span.at,
                                                                                            crate::time::now_ms(),
                                                                                        )}
                                                                                    </span>
                                                                                </span>
                                                                            }
                                                                        })
                                                                        .collect::<Vec<_>>()}
                                                                </div>
                                                            </div>
                                                        }
                                                    })}
                                                </div>
                                            </details>
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
                                            // The old drawer's footer
                                            // actions — stacked,
                                            // left-aligned, icon-led.
                                            <div class="flex flex-col gap-2">
                                                {move || {
                                                    let pinned = summary
                                                        .get()
                                                        .and_then(Result::ok)
                                                        .map_or(pinned_flag, |s| s.pinned);
                                                    view! {
                                                        <Button
                                                            variant=ButtonVariant::Outline
                                                            size=ButtonSize::Sm
                                                            class="w-full justify-start"
                                                            disabled=acting
                                                            on_click=Box::new(move || do_pin(!pinned))
                                                        >
                                                            <Icon name="pin" class="size-4"/>
                                                            {if pinned { "Unpin" } else { "Pin" }}
                                                        </Button>
                                                    }
                                                }}
                                                {move || {
                                                    let archived = summary
                                                        .get()
                                                        .and_then(Result::ok)
                                                        .map_or(archived_flag, |s| s.archived);
                                                    view! {
                                                        <Button
                                                            variant=ButtonVariant::Outline
                                                            size=ButtonSize::Sm
                                                            class="w-full justify-start"
                                                            disabled=acting
                                                            on_click=Box::new(move || do_archive(!archived))
                                                        >
                                                            <Icon name="archive" class="size-4"/>
                                                            {if archived { "Unarchive" } else { "Archive" }}
                                                        </Button>
                                                    }
                                                }}
                                            </div>
                                            // Project membership — the
                                            // node's catalog filtered to
                                            // this session's node.
                                            {move || {
                                                let list = projects
                                                    .get()
                                                    .and_then(Result::ok)
                                                    .unwrap_or_default();
                                                let same_node: Vec<_> = list
                                                    .into_iter()
                                                    .filter(|p| p.node == session_node_opt)
                                                    .collect();
                                                view! {
                                                    <div data-name="ProjectsSection" class="space-y-2">
                                                        <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                                                            "Projects"
                                                        </label>
                                                        {if same_node.is_empty() {
                                                            view! {
                                                                <p class="text-xs text-muted-foreground">
                                                                    "No projects yet."
                                                                </p>
                                                            }
                                                                .into_any()
                                                        } else {
                                                            view! {
                                                                <ul class="space-y-1">
                                                                    {same_node
                                                                        .into_iter()
                                                                        .map(|p| {
                                                                            let key = p.id.clone();
                                                                            let member = move || {
                                                                                summary
                                                                                    .get()
                                                                                    .and_then(Result::ok)
                                                                                    .is_some_and(|s| {
                                                                                        s.project_ids.contains(&key)
                                                                                    })
                                                                            };
                                                                            view! {
                                                                                <li>
                                                                                    <button
                                                                                        type="button"
                                                                                        class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-accent/60"
                                                                                        disabled=move || acting.get()
                                                                                        on:click=move |_| {
                                                                                            toggle_project(p.id.clone());
                                                                                        }
                                                                                    >
                                                                                        <span class="min-w-0 flex-1 truncate">
                                                                                            {p.name.clone()}
                                                                                        </span>
                                                                                        {move || {
                                                                                            member().then(|| {
                                                                                                view! {
                                                                                                    <Icon
                                                                                                        name="check"
                                                                                                        class="size-3.5 shrink-0 text-primary"
                                                                                                    />
                                                                                                }
                                                                                            })
                                                                                        }}
                                                                                    </button>
                                                                                </li>
                                                                            }
                                                                        })
                                                                        .collect::<Vec<_>>()}
                                                                </ul>
                                                            }
                                                                .into_any()
                                                        }}
                                                    </div>
                                                }
                                            }}
                                            <CheckpointList
                                                checkpoints=checkpoints
                                                session_id=session_id()
                                                agent=agent()
                                                on_changed=move || {
                                                    history.refetch();
                                                    summary.refetch();
                                                }
                                            />
                                            // `POST /convert` copies
                                            // the session into another
                                            // agent's store; the
                                            // export link is the node's
                                            // full-IR `GET` through the
                                            // hub's `/api/*` passthrough,
                                            // so a plain anchor downloads
                                            // it same-origin.
                                            <div data-name="ConvertExport" class="space-y-2">
                                                <label class="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                                                    "Convert / Export"
                                                </label>
                                                <p class="text-xs text-muted-foreground">
                                                    "Copy this session into another agent's store, or download the full IR as JSON."
                                                </p>
                                                <div class="flex flex-wrap items-center gap-2">
                                                    {convert_targets(&detail_agent)
                                                        .into_iter()
                                                        .map(|(target, label)| {
                                                            view! {
                                                                <Button
                                                                    variant=ButtonVariant::Outline
                                                                    size=ButtonSize::Sm
                                                                    disabled=acting
                                                                    on_click=Box::new(move || {
                                                                        do_convert(target);
                                                                    })
                                                                >
                                                                    {format!("Convert to {label}")}
                                                                </Button>
                                                            }
                                                        })
                                                        .collect::<Vec<_>>()}
                                                    <a
                                                        data-name="ExportLink"
                                                        class=ButtonClass {
                                                            variant: ButtonVariant::Outline,
                                                            size: ButtonSize::Sm,
                                                        }
                                                        .to_class()
                                                        href=export_href(&detail_id, Some(&detail_agent))
                                                        download=format!("sepia-session-{detail_id}.json")
                                                    >
                                                        "Export JSON"
                                                    </a>
                                                </div>
                                            </div>
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
                                                            on_quote=quote_reply
                                                            rewind_ok=rewind_ok
                                                            on_rewind=on_rewind
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
                                    // class-only. Shows only after the
                                    // stream has been `Open` and then
                                    // dropped (ReconnectTracker — the
                                    // initial connect stays quiet).
                                    // `conn_tracker.update` is a write,
                                    // not a tracked read, so it can't
                                    // re-trigger this class closure.
                                    <div
                                        data-name="ReconnectingPill"
                                        class=move || {
                                            let reconnecting = conn_state
                                                .get()
                                                .is_some_and(|s| {
                                                    let open =
                                                        s.get() == ConnectionReadyState::Open;
                                                    let mut show = false;
                                                    conn_tracker
                                                        .update(|t| show = t.observe(open));
                                                    show
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
                                        <Icon name="arrow-down" class="size-3.5"/>
                                        "Jump to bottom"
                                    </button>
                                    </div>
                                    }.into_any()}
                                    {move || {
                                        running()
                                            .then(|| view! {
                                                <p class="flex items-center gap-2 text-sm">
                                                    <span class="size-1.5 animate-pulse rounded-full bg-info"></span>
                                                    <span class="shimmer-text">"Thinking…"</span>
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
                                        attachments=attachments
                                        area_ref=composer_ref
                                        model=prompt_model.clone()
                                        session_id=session_id()
                                        agent=agent()
                                        on_model_changed=move || summary.refetch()
                                        queued=queued
                                        failed=failed
                                        running=running
                                        stop=do_cancel
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
                on_confirm=move || attach_and_send(true)
            />
            // Per-message rewind — the row's nodeId is the cut point.
            <ConfirmDialog
                open=confirm_rewind
                title="Rewind to this message?"
                body="Session state after this point is removed — every message past it is deleted from the transcript. Files the agent already changed are not touched (use restore for that)."
                confirm_label="Rewind"
                destructive=true
                on_confirm=move || rewind_confirmed.set(true)
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
fn HistoryRow(
    message: HistoryMessageDto,
    dup: usize,
    /// "Reply" action — inserts the message text as a `> ` quote into
    /// the composer draft and focuses it.
    on_quote: impl Fn(&str) + Copy + Send + Sync + 'static,
    /// Whether the session may rewind right now — `canRewind` from the
    /// wire plus not-held/not-busy. Signal so the affordance appears
    /// and disappears as the run state flips.
    rewind_ok: Signal<bool>,
    /// "Rewind" action — the row's `nodeId` is the cut point the
    /// page-level confirm dialog posts.
    on_rewind: impl Fn(i64) + Copy + Send + Sync + 'static,
) -> impl IntoView {
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
        // `group relative` anchors the hover actions strip.
        // `cv-auto` virtualizes off-screen rows — see input.css.
        let class = match role.as_str() {
            "user" => "cv-auto group relative rounded-md border border-info/30 bg-info/5 px-3 py-2",
            _ => "cv-auto group relative rounded-md px-1 py-2",
        };
        // Hover-revealed actions — copy the text, quote it into the
        // composer, or rewind the transcript to this message's node.
        // `rewind_node` is `Some` only for user/assistant rows whose
        // wire `nodeId` arrived; `rewind_ok` drops it while the
        // session is held or a run is in flight.
        let rewind_node = rewind_node_id(&role, message.node_id);
        let copied = RwSignal::new(false);
        // SSR-safe — the callback never fires server-side. `Arc` so
        // the FnMut `actions` closure can lend each render a copy.
        let copy_reset = std::sync::Arc::new(
            leptos_use::use_timeout_fn(move |()| copied.set(false), 1_500.0).start,
        );
        let copy_text = text.clone();
        let quote_text = text.clone();
        let has_text = !text.is_empty();
        let actions = move || {
            (has_text || (rewind_ok.get() && rewind_node.is_some())).then(|| {
                view! {
                    <div
                        data-name="MsgActions"
                        class="absolute right-1.5 top-1.5 flex items-center gap-0.5 rounded-md border bg-popover p-0.5 opacity-0 shadow-sm transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 [@media(hover:none)]:opacity-100"
                    >
                        {has_text.then(|| {
                            // `actions` re-runs when the gate flips —
                            // clone per render instead of moving the
                            // row's text into a one-shot handler.
                            let text = copy_text.clone();
                            let copy_reset = copy_reset.clone();
                            view! {
                                <button
                                    type="button"
                                    aria-label="Copy message"
                                    class="rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground hover:bg-accent hover:text-foreground"
                                    on:click=move |_| {
                                        crate::components::copy_to_clipboard(&text);
                                        copied.set(true);
                                        copy_reset(());
                                    }
                                >
                                    {move || if copied.get() { "copied" } else { "copy" }}
                                </button>
                            }
                        })}
                        {has_text.then(|| {
                            let text = quote_text.clone();
                            view! {
                                <button
                                    type="button"
                                    aria-label="Quote in reply"
                                    title="Quote in reply"
                                    class="rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground hover:bg-accent hover:text-foreground"
                                    on:click=move |_| on_quote(&text)
                                >
                                    "reply"
                                </button>
                            }
                        })}
                        {(rewind_node.is_some() && rewind_ok.get()).then(|| {
                            let node = rewind_node.unwrap_or(0);
                            view! {
                                <button
                                    data-name="MsgRewind"
                                    type="button"
                                    aria-label="Rewind to this message"
                                    title="Rewind to here"
                                    class="rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground hover:bg-accent hover:text-foreground"
                                    on:click=move |_| on_rewind(node)
                                >
                                    "rewind"
                                </button>
                            }
                        })}
                    </div>
                }
            })
        };
        view! {
            <article class=class>
                {actions}
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
        <details data-name="ContextCard" class="cv-auto rounded-lg border border-border/60 bg-muted/30">
            <summary class="group flex cursor-pointer select-none items-center gap-2 px-3 py-2 text-xs text-muted-foreground transition-colors hover:text-foreground [&::-webkit-details-marker]:hidden">
                <span class="font-medium text-foreground/80">"Session context"</span>
                <span class="min-w-0 flex-1 truncate">{summary}</span>
                <Icon
                    name="chevron-down"
                    class="size-3.5 transition-transform group-open:rotate-180"
                />
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
                                    <article class=format!("cv-auto rounded-md border px-3 py-2 {surface}")>
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
                                    "cv-auto rounded-md border border-destructive/50 bg-destructive/10"
                                } else {
                                    "cv-auto rounded-md border border-border/60 bg-muted/40"
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

/// Read one picked/pasted file through `FileReader` into a staged
/// [`Attachment`]. `read_as_data_url` yields `data:<mime>;base64,…`,
/// which [`sepia_web_core::composer::split_data_url`] splits for the
/// ACP block the send builds. Hydrate-only — clipboard/file APIs are
/// browser surfaces.
#[cfg(feature = "hydrate")]
fn read_attachment_file(
    file: &web_sys::File,
    attachments: RwSignal<Vec<Attachment>>,
    toast: crate::components::toast::ToastStore,
) {
    use wasm_bindgen::JsCast;
    let name = file.name();
    let mime = file.type_();
    let size = file.size();
    let Ok(reader) = web_sys::FileReader::new() else {
        toast.error(format!("couldn't read {name}"));
        return;
    };
    // `once_into_js` — the JS function frees its wasm slot after it
    // fires (loadend is once). A `Closure::once` dropped here instead
    // leaves the reader calling a freed trampoline (`unreachable`).
    let done_reader = reader.clone();
    let on_done = wasm_bindgen::closure::Closure::once_into_js(move |_: web_sys::ProgressEvent| {
        match done_reader.result() {
            Ok(v) => match v
                .as_string()
                .and_then(|s| sepia_web_core::composer::split_data_url(&s))
            {
                Some((m, data_b64)) => {
                    // The node 400s over `MAX_ATTACHMENT_CHARS` — refuse
                    // here so the chip never advertises a doomed send.
                    let over = attachments
                        .with(|v| attachment_chars(v) + data_b64.len() > MAX_ATTACHMENT_CHARS);
                    if over {
                        toast.error(format!("{name} exceeds the attachment size limit"));
                    } else {
                        attachments.update(|v| {
                            v.push(Attachment {
                                name,
                                mime_type: if mime.is_empty() { m } else { mime },
                                data_b64,
                                size,
                            });
                        });
                    }
                }
                None => toast.error(format!("couldn't decode {name}")),
            },
            Err(_) => toast.error(format!("couldn't read {name}")),
        }
    });
    reader.set_onloadend(Some(on_done.unchecked_ref()));
    let _ = reader.read_as_data_url(file);
}

/// Stage every file in a `FileList` (picker `change`, paste, drop),
/// honoring the node's part-count cap.
#[cfg(feature = "hydrate")]
fn stage_files(
    files: &web_sys::FileList,
    attachments: RwSignal<Vec<Attachment>>,
    toast: crate::components::toast::ToastStore,
) {
    for i in 0..files.length() {
        let Some(file) = files.get(i) else { continue };
        if attachments.with(Vec::len) >= MAX_ATTACHMENTS {
            toast.error(format!("attachment limit is {MAX_ATTACHMENTS}"));
            return;
        }
        read_attachment_file(&file, attachments, toast);
    }
}

/// `<input type=file>` `change` → stage the selection, then reset the
/// input so picking the same file twice still fires.
#[cfg(feature = "hydrate")]
fn stage_picked(
    ev: &web_sys::Event,
    attachments: RwSignal<Vec<Attachment>>,
    toast: crate::components::toast::ToastStore,
) {
    use wasm_bindgen::JsCast;
    let input = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
    if let Some(input) = input {
        if let Some(files) = input.files() {
            stage_files(&files, attachments, toast);
        }
        input.set_value("");
    }
}
#[cfg(not(feature = "hydrate"))]
fn stage_picked(
    _ev: &leptos::ev::Event,
    _attachments: RwSignal<Vec<Attachment>>,
    _toast: crate::components::toast::ToastStore,
) {
}

/// Textarea `paste` → stage clipboard files (screenshots land as
/// `image/*`). A file-bearing paste eats the default so the image
/// doesn't also dump a name into the draft.
#[cfg(feature = "hydrate")]
fn stage_pasted(
    ev: &leptos::ev::ClipboardEvent,
    attachments: RwSignal<Vec<Attachment>>,
    toast: crate::components::toast::ToastStore,
) {
    let Some(data) = ev.clipboard_data() else {
        return;
    };
    let Some(files) = data.files() else { return };
    if files.length() == 0 {
        return;
    }
    ev.prevent_default();
    stage_files(&files, attachments, toast);
}
#[cfg(not(feature = "hydrate"))]
fn stage_pasted(
    _ev: &leptos::ev::ClipboardEvent,
    _attachments: RwSignal<Vec<Attachment>>,
    _toast: crate::components::toast::ToastStore,
) {
}

/// Composer `dragover` — only file payloads light the drop ring;
/// anything else is left to the textarea (old `PromptInput` behavior).
#[cfg(feature = "hydrate")]
fn on_dragover(ev: &leptos::ev::DragEvent, dragging: RwSignal<bool>) {
    let Some(dt) = ev.data_transfer() else { return };
    let files = dt
        .types()
        .iter()
        .any(|t| t.as_string().as_deref() == Some("Files"));
    if files {
        ev.prevent_default();
        dragging.set(true);
    }
}
#[cfg(not(feature = "hydrate"))]
fn on_dragover(_ev: &leptos::ev::DragEvent, _dragging: RwSignal<bool>) {}

/// `dragleave` — crossing into a child isn't leaving the box, so the
/// ring only clears when the pointer exits the container.
#[cfg(feature = "hydrate")]
fn on_dragleave(ev: &leptos::ev::DragEvent, dragging: RwSignal<bool>) {
    use wasm_bindgen::JsCast;
    let inside = ev
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .zip(
            ev.related_target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok()),
        )
        .is_some_and(|(el, rel)| el.contains(Some(&rel)));
    if !inside {
        dragging.set(false);
    }
}
#[cfg(not(feature = "hydrate"))]
fn on_dragleave(_ev: &leptos::ev::DragEvent, _dragging: RwSignal<bool>) {}

/// `drop` → stage the payload like a picker selection, then clear the
/// ring.
#[cfg(feature = "hydrate")]
fn on_drop(
    ev: &leptos::ev::DragEvent,
    attachments: RwSignal<Vec<Attachment>>,
    dragging: RwSignal<bool>,
    toast: crate::components::toast::ToastStore,
) {
    dragging.set(false);
    let Some(dt) = ev.data_transfer() else { return };
    let Some(files) = dt.files() else { return };
    if files.length() == 0 {
        return;
    }
    ev.prevent_default();
    stage_files(&files, attachments, toast);
}
#[cfg(not(feature = "hydrate"))]
fn on_drop(
    _ev: &leptos::ev::DragEvent,
    _attachments: RwSignal<Vec<Attachment>>,
    _dragging: RwSignal<bool>,
    _toast: crate::components::toast::ToastStore,
) {
}

/// The prompt composer — the old app's `PromptInput` anatomy: one
/// bordered input-group box holding the staged-attachment chips, a
/// borderless auto-growing textarea (Enter sends, Shift+Enter a
/// newline, IME composition ignored), and a footer strip — paperclip
/// tool left; editable model override (`patch_meta {model}` — the wire
/// carries no per-agent model list, so it's a free-text field rather
/// than the old settings-driven select; empty restores the agent
/// default) + send right. The send button flips to a spinner while the
/// prompt POST is in flight and to a stop square while a run streams —
/// pressing it cancels. `queued`/`failed` are this session's outbox
/// counts, surfaced while writes wait on a down node. Dragging files
/// anywhere over the box rings it like the old drop target.
#[component]
#[allow(clippy::too_many_arguments)]
fn PromptBox(
    draft: RwSignal<String>,
    sending: RwSignal<bool>,
    attachments: RwSignal<Vec<Attachment>>,
    /// Page-owned textarea ref — the transcript's quote action focuses
    /// through it.
    area_ref: NodeRef<leptos::html::Textarea>,
    /// The session's meta `model` — the input's baseline; a local edit
    /// overrides it until the next refetch.
    model: Option<String>,
    session_id: String,
    agent: Option<String>,
    /// Re-fetch the summary after a model patch lands.
    on_model_changed: impl Fn() + 'static + Send + Sync + Copy,
    queued: usize,
    failed: usize,
    /// A run is streaming on the live feed — the send button becomes
    /// Stop.
    running: impl Fn() -> bool + 'static + Send + Sync + Copy,
    /// `POST /cancel` — the stop button's action.
    stop: impl Fn() + 'static + Send + Sync + Copy,
    submit: impl Fn() + 'static + Send + Sync + Copy,
) -> impl IntoView {
    let owner = StoredValue::new(Owner::current().unwrap_or_default());
    let toast = use_toast();
    let file_ref = NodeRef::<leptos::html::Input>::new();
    // File drop ring — hydrate-only signal; SSR renders the base box.
    let dragging = RwSignal::new(false);
    // `None` = no local edit — the input mirrors the wire's `model`.
    let model_draft: RwSignal<Option<String>> = RwSignal::new(None);
    let wire_model = model.clone().unwrap_or_default();
    // Cloned for each binding — the closure captures owned Strings
    // (session id / agent / wire model), so it's `Clone`, not `Copy`.
    let commit_model = move || {
        let Some(raw) = model_draft.get_untracked() else {
            return;
        };
        model_draft.set(None);
        let v = raw.trim().to_string();
        // A no-op commit (unchanged text) skips the PATCH.
        if v == wire_model {
            return;
        }
        let next = (!v.is_empty()).then_some(v);
        let ok = match &next {
            Some(m) => format!("Model set to {m}"),
            None => "Model reset to agent default".to_string(),
        };
        let id = session_id.clone();
        let agent = agent.clone();
        crate::app::spawn_under(owner.get_value(), async move {
            if toast
                .outcome(set_session_model(id, agent, next).await, ok)
                .is_some()
            {
                on_model_changed();
            }
        });
    };
    let commit_model_enter = commit_model.clone();
    // Enter sends; Shift+Enter inserts a newline; an active IME
    // composition owns Enter entirely.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
            ev.prevent_default();
            submit();
        }
    };
    view! {
        <div class="space-y-2">
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
            // The input-group box — chips + textarea + footer strip
            // share one border; focus rings the group, not the
            // textarea.
            <div
                role="group"
                data-name="PromptInput"
                class=move || {
                    let base = "group/input-group relative flex w-full min-w-0 flex-col overflow-hidden rounded-2xl border border-transparent bg-input/50 transition-[color,box-shadow,background-color] has-[textarea:focus-visible]:ring-2 has-[textarea:focus-visible]:ring-ring";
                    if dragging.get() {
                        format!("{base} border-ring ring-3 ring-ring/30")
                    } else {
                        base.to_string()
                    }
                }
                on:dragover=move |ev| on_dragover(&ev, dragging)
                on:dragleave=move |ev| on_dragleave(&ev, dragging)
                on:drop=move |ev| on_drop(&ev, attachments, dragging, toast)
            >
                {move || {
                    let items = attachments.get();
                    (!items.is_empty()).then(|| {
                        view! {
                            <div
                                data-name="AttachmentStrip"
                                class="flex flex-wrap items-center gap-1.5 px-3 pt-2.5"
                            >
                                {items
                                    .iter()
                                    .enumerate()
                                    .map(|(i, a)| {
                                        let size = byte_label(a.size);
                                        let tip = format!("{} · {size}", a.name);
                                        view! {
                                            <span
                                                class="inline-flex max-w-56 items-center gap-2 rounded-lg border border-border/60 bg-muted/40 py-1.5 pr-1.5 pl-1.5 text-xs"
                                                title=tip
                                            >
                                                <span class="flex size-7 shrink-0 items-center justify-center rounded-md bg-background text-muted-foreground">
                                                    <Icon name="file" class="size-4"/>
                                                </span>
                                                <span class="flex min-w-0 flex-col leading-tight">
                                                    <span class="truncate font-medium text-foreground/90">
                                                        {a.name.clone()}
                                                    </span>
                                                    <span class="truncate text-muted-foreground/70">
                                                        {size}
                                                    </span>
                                                </span>
                                                <button
                                                    type="button"
                                                    aria-label=format!("Remove {}", a.name)
                                                    class="shrink-0 rounded p-0.5 text-muted-foreground/70 hover:bg-muted hover:text-foreground"
                                                    on:click=move |_| {
                                                        attachments.update(|v| {
                                                            v.remove(i);
                                                        });
                                                    }
                                                >
                                                    <Icon name="x" class="size-3.5"/>
                                                </button>
                                            </span>
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </div>
                        }
                    })
                }}
                <textarea
                    class="field-sizing-content max-h-48 min-h-16 w-full resize-none border-0 bg-transparent px-3 py-2.5 text-base shadow-none outline-none placeholder:text-muted-foreground md:text-sm"
                    placeholder="Prompt the agent…"
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                    on:keydown=on_keydown
                    on:paste=move |ev| stage_pasted(&ev, attachments, toast)
                    rows=1
                    node_ref=area_ref
                ></textarea>
                // Footer strip — paperclip tool left, model + send right.
                <div class="flex w-full items-center justify-between gap-1 px-3 pb-3">
                    <div class="flex min-w-0 items-center gap-1">
                        <button
                            type="button"
                            aria-label="Attach files"
                            title="Attach files"
                            class="inline-flex h-6 items-center gap-1 rounded-xl px-1.5 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            on:click=move |_| {
                                if let Some(el) = file_ref.get() {
                                    el.click();
                                }
                            }
                        >
                            <Icon name="paperclip" class="size-3.5"/>
                        </button>
                        // Hidden picker — markup identical SSR/hydrate,
                        // the handlers are hydrate-only behavior.
                        <input
                            node_ref=file_ref
                            type="file"
                            multiple=true
                            class="hidden"
                            data-name="AttachmentInput"
                            on:change=move |ev| stage_picked(&ev, attachments, toast)
                        />
                    </div>
                    <div class="ml-auto flex min-w-0 items-center gap-1">
                        <input
                            type="text"
                            data-name="ModelInput"
                            class="h-7 w-28 min-w-0 max-w-40 border-0 bg-transparent px-2 font-mono text-xs text-muted-foreground transition-colors placeholder:text-muted-foreground/50 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                            placeholder="agent default"
                            title="Model for the next agent spawn — clear to restore the agent default"
                            maxlength="100"
                            prop:value=move || {
                                model_draft.get().unwrap_or_else(|| model.clone().unwrap_or_default())
                            }
                            on:input=move |ev| model_draft.set(Some(event_target_value(&ev)))
                            on:change=move |_| commit_model()
                            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                if ev.key() == "Enter" {
                                    ev.prevent_default();
                                    commit_model_enter();
                                }
                            }
                        />
                        <button
                            type="button"
                            data-name="SendButton"
                            class="inline-flex size-8 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
                            aria-label=move || if running() { "Stop" } else { "Send" }
                            title=move || if running() { "Stop" } else { "Send" }
                            // The old submit disabled on an empty
                            // draft; a running session keeps the
                            // button alive — it's the stop control.
                            disabled=move || {
                                sending.get()
                                    || (!running()
                                        && draft.read().trim().is_empty()
                                        && attachments.read().is_empty())
                            }
                            on:click=move |_| {
                                if running() {
                                    stop();
                                } else {
                                    submit();
                                }
                            }
                        >
                            {move || {
                                if sending.get() {
                                    view! {
                                        <Icon name="loader-circle" class="size-4 animate-spin"/>
                                    }
                                } else if running() {
                                    view! { <Icon name="square" class="size-4"/> }
                                } else {
                                    view! { <Icon name="send" class="size-4"/> }
                                }
                            }}
                        </button>
                    </div>
                </div>
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
    let owner = StoredValue::new(Owner::current().unwrap_or_default());
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
        crate::app::spawn_under(owner.get_value(), async move {
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

/// One `label: value` row in the details sheet — the old drawer's
/// `Detail`. `copy` adds a ghost clipboard button that flashes a ✓.
#[component]
fn DetailRow(
    #[prop(into)] label: String,
    /// When set, a copy button writes this to the clipboard.
    #[prop(into, optional)]
    copy: String,
    /// Render the value in mono (ids, paths).
    #[prop(optional)]
    mono: bool,
    children: Children,
) -> impl IntoView {
    #[allow(unused)]
    let owner = StoredValue::new(Owner::current().unwrap_or_default());
    let copied = RwSignal::new(false);
    let copy_tip = format!("Copy {label}");
    let value_cls = if mono {
        "min-w-0 flex-1 truncate font-mono text-xs"
    } else {
        "min-w-0 flex-1 truncate"
    };
    view! {
        <div class="flex flex-col gap-0.5 py-2 text-sm">
            <span class="text-xs text-muted-foreground">{label}</span>
            <span class="flex min-w-0 items-center gap-1">
                <span class=value_cls title=copy.clone()>{children()}</span>
                {(!copy.is_empty()).then(|| {
                    view! {
                        <button
                            type="button"
                            class="inline-flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            aria-label=copy_tip.clone()
                            title=copy_tip
                            on:click=move |_| {
                                crate::components::copy_to_clipboard(&copy);
                                copied.set(true);
                                #[cfg(feature = "hydrate")]
                                crate::app::spawn_under(owner.get_value(), async move {
                                    crate::sleep_ms(1200).await;
                                    copied.set(false);
                                });
                            }
                        >
                            {move || {
                                if copied.get() {
                                    view! { <Icon name="check" class="size-3.5 text-success"/> }
                                } else {
                                    view! { <Icon name="copy" class="size-3.5"/> }
                                }
                            }}
                        </button>
                    }
                })}
            </span>
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
    let owner = StoredValue::new(Owner::current().unwrap_or_default());
    let busy = RwSignal::new(false);
    let confirm_restore = RwSignal::new(false);
    let confirm_rewind = RwSignal::new(false);
    let checkpoint_ref = checkpoint.r#ref.clone();
    let toast = use_toast();
    let run = move |op: &'static str| {
        if busy.get() {
            return;
        }
        busy.set(true);
        let id = session_id.clone();
        let agent = agent.clone();
        let checkpoint = checkpoint_ref.clone();
        crate::app::spawn_under(owner.get_value(), async move {
            let changed = if op == "restore" {
                toast
                    .outcome(restore_checkpoint(id, agent, checkpoint).await, "Restored.")
                    .is_some()
            } else {
                match rewind_session(id, agent, RewindSelector::Checkpoint(checkpoint)).await {
                    Ok(r) => {
                        toast.success(rewind_result_summary(r.removed));
                        true
                    }
                    Err(e) => {
                        toast.error(e.to_string());
                        false
                    }
                }
            };
            if changed {
                on_changed();
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
