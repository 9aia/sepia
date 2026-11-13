//! feed — the live event feed: a second `sepia serve` child
//! subscribed to `events.subscribe`, buffering `node.event`
//! notifications for the `live`/`stream`/`permissions` queries and
//! the `jobs`/`notifications` platform feeds.
//!
//! Why a second child: [`crate::data`]'s client is request/response
//! — its reader skips non-matching lines, which would silently eat
//! the notification stream. The feed child owns its stdout as a
//! pure stream: one `events.subscribe` request (id 1), the ack
//! response, then `node.event` notifications forever. It shares the
//! data child's target resolution ([`crate::data::resolve_target`]
//! and [`crate::data::spawn_serve`]) but resolves against an empty
//! param map — `--param sepiaBin=` steers the request lane only;
//! the feed follows `$SEPIA_BIN`/sibling/self resolution.
//!
//! ## Lifecycle
//!
//! [`warm`] (called from `serve` alongside `data::warm`) spawns a
//! supervisor thread that loops: spawn → subscribe → read until
//! EOF/error → mark `stale` → backoff → respawn. The ack resets the
//! failure count, so a long-lived feed that drops reconnects
//! immediately; a child that never subscribes backs off 1s→5s and
//! gives up after [`MAX_FAILURES`] consecutive failures (`down`).
//! [`ensure`] — called by every feed query — re-arms a `down`
//! supervisor, so recovery is lazy (next query) rather than a
//! polling thread. The event buffer survives reconnects: a respawn
//! resumes appending to the same [`Buffer`].
//!
//! The child is reaped by [`FeedProc`]'s `Drop` at each attempt's
//! end; on driver exit the process's fds close, the child's stdin
//! sees EOF, and `sepia serve` exits on its own — same contract as
//! the data child.
//!
//! ## Wire shapes (parsed as `serde_json::Value`, not typed)
//!
//! `node.event` params are `sepia_session::NodeEvent` serializations
//! (`#[serde(tag = "type", rename_all = "camelCase")]`):
//! `attached`/`created`/`detached`/`deleted` carry
//! `sessionId`+`agentId`(+`cwd`/`reason`), `sessionEvents` carries
//! `sessionId`+`events: [SessionEvent]`, `lagged` carries
//! `sessionId?`+`count`. `SessionEvent` frames are
//! `sepia_proto::SessionEvent` (`runStarted`, `textMessage*`,
//! `reasoningMessage*`, `toolCall*`, `custom`, `runFinished`).
//! Permission asks arrive as `custom` events named
//! `acp:permission_request` whose `value` is a serialized
//! `sepia_acp::PermissionRequest` (`requestId`, `title`,
//! `toolKind`/`toolCallId`, `options[{optionId,name,kind}]`).
//!
//! The wire carries **no timestamps** — `lastEvent`/`since`/`age`
//! are the driver's receive times (`SystemTime::now` at ingest).
//!
//! ## Buffer bounds
//!
//! Per session: [`EVENTS_PER_SESSION`] frames or
//! [`SESSION_BYTES`] of serialized JSON, whichever hits first —
//! oldest dropped first. [`MAX_SESSIONS`] sessions tracked, evicting
//! the stalest. [`NODE_MARKERS`] lifecycle markers ring for
//! `feed.status`. `Lagged` notifications are never dropped: they
//! bump `lagged` counters surfaced by `feed.status`/`permissions`.
//!
//! ## Pending-permission rule
//!
//! A `session/request_permission` is pending from its
//! `acp:permission_request` frame until the first of:
//!
//! 1. `sessions.permission` succeeds through [`resolve_permission`]
//!    (called by `ops::serve_invoke` on a wire-ok reply),
//! 2. `runFinished` for the session — the turn ended, so any
//!    unanswered ask is dead (agents block the turn on the reply),
//! 3. `detached`/`deleted` — `respond_to_permission` requires a
//!    live attach (`require_live`), so a detached ask is
//!    unanswerable; a re-attach makes the agent re-issue.
//!
//! Queries additionally hide asks older than [`PERMISSION_TTL`]
//! (1h) — belt-and-suspenders against a wedged feed.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use sepia_driver::rpc::{self, Inbound};

use crate::data;
use crate::views::cell_text;

/// `events.subscribe` is the feed child's only request — id 1, so
/// the ack is the first response line and every later line is a
/// notification (or noise, skipped).
const SUBSCRIBE_ID: u64 = 1;

/// Per-session frame cap — the `stream` tail never needs more than
/// a few hundred frames of delta context.
const EVENTS_PER_SESSION: usize = 500;
/// Per-session serialized-size cap (~1 MiB) — bounds memory when a
/// session streams huge tool outputs faster than the count cap.
const SESSION_BYTES: usize = 1024 * 1024;
/// Lifecycle markers kept for `feed.status`'s `recent` list.
const NODE_MARKERS: usize = 100;
/// Distinct sessions tracked — beyond this the stalest is evicted.
const MAX_SESSIONS: usize = 256;
/// Consecutive subscribe failures before the supervisor gives up
/// (`down`); [`ensure`] re-arms on the next feed query.
const MAX_FAILURES: u32 = 5;
/// Respawn backoff — doubles per consecutive failure, capped.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// Pending asks older than this are hidden from queries (see the
/// module docs' resolution rule).
const PERMISSION_TTL: Duration = Duration::from_secs(3600);

// ------------------------------------------------------------------ buffer

/// Feed liveness — what `feed.status` reports and what `ensure`
/// re-arms on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    /// Supervisor spawned, subscribe not yet acked.
    #[default]
    Starting,
    /// Subscribe acked, stream reading.
    Live,
    /// Stream ended/errored — supervisor backing off, buffer kept.
    Stale,
    /// `MAX_FAILURES` consecutive failures — supervisor exited;
    /// `ensure` restarts it on the next feed query.
    Down,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Live => "live",
            Self::Stale => "stale",
            Self::Down => "down",
        }
    }
}

/// One pending `session/request_permission` — extracted from an
/// `acp:permission_request` custom frame.
#[derive(Clone, Debug)]
struct PendingPermission {
    request_id: String,
    title: String,
    /// `toolKind` when the agent reported one, else `toolCallId`,
    /// else `""` — the "what is it asking about" column.
    tool: String,
    /// The wire `options[]` verbatim (`{optionId,name,kind}`).
    options: Vec<Value>,
    since: SystemTime,
}

/// A buffered `SessionEvent` frame plus its receive time and
/// serialized size (for the byte cap).
type FeedEvent = (SystemTime, Value, usize);

/// Per-session feed state.
#[derive(Default)]
struct SessionFeed {
    /// Last `agentId` seen on a lifecycle marker — `SessionEvents`
    /// batches don't carry it.
    agent: Option<String>,
    /// Live-map membership per the last attach/detach marker.
    attached: bool,
    /// `runStarted` seen without a matching `runFinished`.
    running: bool,
    events: VecDeque<FeedEvent>,
    event_bytes: usize,
    last_event: Option<SystemTime>,
    /// `Lagged` counts attributed to this session's channel.
    lagged: u64,
    /// `requestId → pending ask`.
    pending: HashMap<String, PendingPermission>,
}

impl SessionFeed {
    fn push_event(&mut self, ts: SystemTime, ev: &Value) {
        let bytes = serde_json::to_string(ev).map_or(0, |s| s.len());
        self.events.push_back((ts, ev.clone(), bytes));
        self.event_bytes += bytes;
        self.last_event = Some(ts);
        while self.events.len() > EVENTS_PER_SESSION || self.event_bytes > SESSION_BYTES {
            if let Some((_, _, b)) = self.events.pop_front() {
                self.event_bytes = self.event_bytes.saturating_sub(b);
            } else {
                break;
            }
        }
    }
}

/// The whole feed buffer — behind one `RwLock`; readers (queries)
/// take `read`, the reader thread `write`.
#[derive(Default)]
struct Buffer {
    sessions: HashMap<String, SessionFeed>,
    /// Recent lifecycle markers (`{type, sessionId, agentId?, at}`)
    /// for `feed.status.recent`.
    markers: VecDeque<Value>,
    /// Node-feed `Lagged` total (`sessionId: null` counts here too).
    lagged_total: u64,
    phase: Phase,
    detail: Option<String>,
    /// Consecutive subscribe failures since the last ack.
    failures: u32,
    /// Last line received of any kind — feed liveness hint.
    last_line: Option<SystemTime>,
}

impl Buffer {
    fn session_mut(&mut self, id: &str) -> &mut SessionFeed {
        if !self.sessions.contains_key(id) && self.sessions.len() >= MAX_SESSIONS {
            // Evict the stalest session — oldest last_event wins;
            // sessions that never saw an event sort first.
            if let Some(oldest) = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.last_event.unwrap_or(SystemTime::UNIX_EPOCH))
                .map(|(k, _)| (*k).clone())
            {
                self.sessions.remove(&oldest);
            }
        }
        self.sessions.entry(id.to_string()).or_default()
    }

    fn marker(&mut self, kind: &str, session_id: &str, agent_id: Option<&str>) {
        let mut m = json!({
            "type": kind,
            "sessionId": session_id,
            "at": rfc3339(SystemTime::now()),
        });
        if let Some(a) = agent_id {
            m["agentId"] = json!(a);
        }
        self.markers.push_back(m);
        while self.markers.len() > NODE_MARKERS {
            self.markers.pop_front();
        }
    }

    /// Ingest one `node.event` notification's `params` (the
    /// `NodeEvent` JSON). Malformed/unknown shapes are ignored —
    /// the feed must never die on a bad frame.
    fn ingest(&mut self, params: &Value) {
        self.last_line = Some(SystemTime::now());
        let sid = || {
            params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match params.get("type").and_then(Value::as_str) {
            Some("attached" | "created") => {
                let id = sid();
                if id.is_empty() {
                    return;
                }
                let agent = params.get("agentId").and_then(Value::as_str);
                let s = self.session_mut(&id);
                s.attached = true;
                if let Some(a) = agent.filter(|a| !a.is_empty()) {
                    s.agent = Some(a.to_string());
                }
                self.marker(params["type"].as_str().unwrap_or_default(), &id, agent);
            }
            Some("detached" | "deleted") => {
                let id = sid();
                if id.is_empty() {
                    return;
                }
                let kind = params["type"].as_str().unwrap_or_default();
                let agent = params.get("agentId").and_then(Value::as_str);
                {
                    let s = self.session_mut(&id);
                    s.attached = false;
                    // Pending asks die with the attach — see the
                    // module docs' resolution rule.
                    s.pending.clear();
                    if let Some(a) = agent.filter(|a| !a.is_empty()) {
                        s.agent = Some(a.to_string());
                    }
                }
                self.marker(kind, &id, agent);
            }
            Some("sessionEvents") => {
                let id = sid();
                if id.is_empty() {
                    return;
                }
                let events = params
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let ts = SystemTime::now();
                let s = self.session_mut(&id);
                for ev in &events {
                    s.push_event(ts, ev);
                    match ev.get("type").and_then(Value::as_str) {
                        Some("runStarted") => s.running = true,
                        Some("runFinished") => {
                            s.running = false;
                            s.pending.clear();
                        }
                        Some("custom") => {
                            if ev.get("name").and_then(Value::as_str)
                                == Some("acp:permission_request")
                                && let Some(p) = permission_request(ev)
                            {
                                s.pending.insert(p.request_id.clone(), p);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("lagged") => {
                let count = params.get("count").and_then(Value::as_u64).unwrap_or(0);
                self.lagged_total += count;
                let id = params.get("sessionId").and_then(Value::as_str);
                match id {
                    Some(sid) if !sid.is_empty() => {
                        self.session_mut(sid).lagged += count;
                        self.marker("lagged", sid, None);
                    }
                    _ => {
                        // Node-feed overflow — no session to mark.
                        self.markers.push_back(json!({
                            "type": "lagged",
                            "count": count,
                            "at": rfc3339(SystemTime::now()),
                        }));
                        while self.markers.len() > NODE_MARKERS {
                            self.markers.pop_front();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The `events.subscribe` ack's `live` snapshot — ids already
    /// attached at subscribe time (they predate the stream).
    fn mark_live(&mut self, ids: &[String]) {
        for id in ids {
            self.session_mut(id).attached = true;
        }
    }

    fn set_phase(&mut self, phase: Phase, detail: Option<String>) {
        self.phase = phase;
        self.detail = detail;
    }
}

/// Extract a [`PendingPermission`] from an `acp:permission_request`
/// custom frame — `None` when the payload lacks a `requestId`.
/// Pure — unit-tested with synthetic JSON.
fn permission_request(ev: &Value) -> Option<PendingPermission> {
    let value = ev.get("value")?;
    let request_id = value.get("requestId")?.as_str()?.to_string();
    let tool = value
        .get("toolKind")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            value
                .get("toolCallId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default()
        .to_string();
    Some(PendingPermission {
        request_id,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tool,
        options: value
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        since: SystemTime::now(),
    })
}

/// `SystemTime` → RFC 3339 (the platform feeds' timestamp shape).
fn rfc3339(t: SystemTime) -> String {
    time::OffsetDateTime::from(t)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn age_secs(since: SystemTime) -> u64 {
    since.elapsed().map_or(0, |d| d.as_secs())
}

// ------------------------------------------------------------- snapshots

/// Live-session rows for the `live` query — feed state only (the
/// view merges `sessions.list` fields on top). Sorted by most recent
/// activity. Pure snapshot — unit-tested via [`Buffer`].
fn live_rows(buffer: &Buffer) -> Vec<Value> {
    let mut rows: Vec<(&String, &SessionFeed)> = buffer.sessions.iter().collect();
    rows.sort_by(|a, b| {
        b.1.last_event
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .cmp(&a.1.last_event.unwrap_or(SystemTime::UNIX_EPOCH))
            .then_with(|| a.0.cmp(b.0))
    });
    rows.into_iter()
        .map(|(id, s)| {
            let status = if s.running {
                "busy"
            } else if s.attached {
                "attached"
            } else {
                "detached"
            };
            json!({
                "id": id,
                "agent": s.agent.as_deref(),
                "status": status,
                "attached": s.attached,
                "running": s.running,
                "lastEvent": s.last_event.map(rfc3339),
                "pending": s.pending.len(),
                "lagged": s.lagged,
                "events": s.events.len(),
            })
        })
        .collect()
}

/// `(session_id, agent)` pairs for sessions mid-turn — the `jobs`
/// feed's busy list.
fn busy_sessions(buffer: &Buffer) -> Vec<(String, String)> {
    buffer
        .sessions
        .iter()
        .filter(|(_, s)| s.running)
        .map(|(id, s)| (id.clone(), s.agent.clone().unwrap_or_default()))
        .collect()
}

/// Pending permission rows for the `permissions` query — asks older
/// than [`PERMISSION_TTL`] are hidden (see module docs). Oldest
/// first.
fn permission_rows(buffer: &Buffer) -> Vec<Value> {
    let now = SystemTime::now();
    let mut rows: Vec<(&String, &SessionFeed, &PendingPermission)> = buffer
        .sessions
        .iter()
        .flat_map(|(id, s)| s.pending.values().map(move |p| (id, s, p)))
        .filter(|(_, _, p)| now.duration_since(p.since).unwrap_or_default() < PERMISSION_TTL)
        .collect();
    rows.sort_by_key(|(_, _, p)| p.since);
    rows.into_iter()
        .map(|(id, s, p)| {
            let options = p
                .options
                .iter()
                .map(|o| {
                    let oid = o["optionId"].as_str().unwrap_or_default();
                    let kind = o["kind"].as_str().unwrap_or_default();
                    if kind.is_empty() {
                        oid.to_string()
                    } else {
                        format!("{oid} ({kind})")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            json!({
                "id": id,
                "agent": s.agent.as_deref(),
                "request": p.request_id.as_str(),
                "title": p.title.as_str(),
                "tool": p.tool.as_str(),
                "options": options,
                "age": age_secs(p.since),
                "since": rfc3339(p.since),
            })
        })
        .collect()
}

/// The `feed.status` query payload.
fn status_value(buffer: &Buffer) -> Value {
    json!({
        "status": buffer.phase.as_str(),
        "detail": buffer.detail.as_deref(),
        "failures": buffer.failures,
        "sessions": buffer.sessions.len(),
        "pendingPermissions": buffer
            .sessions
            .values()
            .map(|s| s.pending.len())
            .sum::<usize>(),
        "lagged": buffer.lagged_total,
        "lastEvent": buffer.last_line.map(rfc3339),
        "recent": buffer.markers.iter().rev().take(10).collect::<Vec<_>>(),
    })
}

/// Flatten buffered `SessionEvent` frames into `history`-shaped
/// rows (`role`/`time`/`node`/`tool`/`content`). Streaming frames
/// are deltas — text/reasoning/tool-arg deltas accumulate per open
/// message/tool-call and flush on the `*End`/`toolCallResult`/
/// `toolCallEnd` frame (or at the tail, for a mid-flight cut).
/// `time` is the frame's receive time (the wire carries none).
/// Pure — unit-tested with synthetic frames.
fn flatten_events(events: &[FeedEvent]) -> Vec<Value> {
    // message_id → (role, accumulated text); tool_call_id → (name, args)
    let mut texts: HashMap<String, (String, String)> = HashMap::new();
    let mut tools: HashMap<String, (String, String)> = HashMap::new();
    let mut rows: Vec<Value> = Vec::new();

    let mut push = |role: &str, time: String, tool: &str, content: &str| {
        rows.push(json!({
            "role": role,
            "time": time,
            "node": Value::Null,
            "tool": tool,
            "content": cell_text(&Value::String(content.to_string())),
        }));
    };

    for (ts, ev, _) in events {
        let time = rfc3339(*ts);
        let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
        let id = |key: &str| {
            ev.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match kind {
            "runStarted" => push("run", time, "", "run started"),
            "runFinished" => push("run", time, "", "run finished"),
            "textMessageStart" | "reasoningMessageStart" => {
                let role = if kind.starts_with("reasoning") {
                    "reasoning"
                } else {
                    ev.get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("assistant")
                };
                texts.insert(id("messageId"), (role.to_string(), String::new()));
            }
            "textMessageContent" | "reasoningMessageContent" => {
                if let Some((_, buf)) = texts.get_mut(&id("messageId"))
                    && let Some(delta) = ev.get("delta").and_then(Value::as_str)
                {
                    buf.push_str(delta);
                }
            }
            "textMessageEnd" | "reasoningMessageEnd" => {
                if let Some((role, buf)) = texts.remove(&id("messageId")) {
                    push(&role, time, "", &buf);
                }
            }
            "toolCallStart" => {
                tools.insert(id("toolCallId"), (id("toolCallName"), String::new()));
            }
            "toolCallArgs" => {
                if let Some((_, buf)) = tools.get_mut(&id("toolCallId"))
                    && let Some(delta) = ev.get("delta").and_then(Value::as_str)
                {
                    buf.push_str(delta);
                }
            }
            "toolCallResult" => {
                let name = tools
                    .get(&id("toolCallId"))
                    .map_or_else(|| id("toolCallId"), |(n, _)| n.clone());
                let content = ev
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                push("tool", time, &name, content);
            }
            "toolCallEnd" => {
                let tcid = id("toolCallId");
                let (name, buf) = tools.remove(&tcid).unwrap_or_else(|| {
                    (
                        ev.get("toolCallName")
                            .and_then(Value::as_str)
                            .unwrap_or(&tcid)
                            .to_string(),
                        String::new(),
                    )
                });
                let status = ev.get("status").and_then(Value::as_str).unwrap_or("");
                let content = if buf.is_empty() {
                    status.to_string()
                } else {
                    buf
                };
                push("tool", time, &name, &content);
            }
            "custom" => {
                let name = ev.get("name").and_then(Value::as_str).unwrap_or_default();
                if name == "acp:permission_request" {
                    let title = ev
                        .get("value")
                        .and_then(|v| v.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("permission requested");
                    let options = ev
                        .get("value")
                        .and_then(|v| v.get("options"))
                        .and_then(Value::as_array)
                        .map(|os| {
                            os.iter()
                                .filter_map(|o| o["optionId"].as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let content = if options.is_empty() {
                        title.to_string()
                    } else {
                        format!("{title} — options: {options}")
                    };
                    push("permission", time, title, &content);
                } else {
                    push("custom", time, "", name);
                }
            }
            _ => {}
        }
    }
    // Flush frames still open at the tail — the cap can cut a
    // message/tool call mid-flight.
    for (_, (role, buf)) in texts {
        if !buf.is_empty() {
            push(&role, rfc3339(SystemTime::now()), "", &buf);
        }
    }
    for (_, (name, buf)) in tools {
        if !buf.is_empty() {
            push("tool", rfc3339(SystemTime::now()), &name, &buf);
        }
    }
    rows
}

// ------------------------------------------------------------- child lane

/// The live feed child — `child`+`stdin` parked here so a dropped
/// attempt reaps the process; `stdout` lives in the reader loop.
struct FeedProc {
    child: Child,
    #[allow(dead_code)]
    stdin: ChildStdin,
}

impl Drop for FeedProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Shared feed state — the buffer plus liveness bookkeeping.
struct Shared {
    buffer: RwLock<Buffer>,
    /// The in-flight attempt's child handles (reaped on attempt end).
    proc: Mutex<Option<FeedProc>>,
    stop: AtomicBool,
    /// One supervisor thread at a time — `ensure` re-arms only when
    /// the previous one exited.
    supervisor_active: AtomicBool,
}

/// Process-wide feed slot — `Vacant` until [`warm`]/[`ensure`],
/// `Ready` while a supervisor (running or re-armable) exists.
enum Slot {
    Vacant,
    Ready(Arc<Shared>),
}

static FEED: Mutex<Slot> = Mutex::new(Slot::Vacant);

fn shared() -> Option<Arc<Shared>> {
    match &*FEED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        Slot::Ready(s) => Some(Arc::clone(s)),
        Slot::Vacant => None,
    }
}

/// Arm the feed: create the shared state and spawn the supervisor
/// thread. Idempotent — a live supervisor is left alone, a `down`
/// one is re-armed (the buffer survives).
pub fn ensure() {
    let shared = {
        let mut slot = FEED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*slot {
            Slot::Ready(s) => Arc::clone(s),
            Slot::Vacant => {
                let s = Arc::new(Shared {
                    buffer: RwLock::new(Buffer::default()),
                    proc: Mutex::new(None),
                    stop: AtomicBool::new(false),
                    supervisor_active: AtomicBool::new(false),
                });
                *slot = Slot::Ready(Arc::clone(&s));
                s
            }
        }
    };
    spawn_supervisor(&shared);
}

/// Spawn `sepia serve` + `events.subscribe` in the background at
/// `serve` start — same role as [`crate::data::warm`]: the plane
/// build overlaps the host's handshake. Best-effort; failures
/// surface as `feed.status` and retry with backoff.
pub fn warm() {
    ensure();
}

fn spawn_supervisor(shared: &Arc<Shared>) {
    if shared.supervisor_active.swap(true, Ordering::SeqCst) {
        return;
    }
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        supervisor(&shared);
        shared.supervisor_active.store(false, Ordering::SeqCst);
    });
}

/// The respawn loop — see the module docs for the backoff/give-up
/// rules. Runs until `stop` (never set today — process exit is the
/// teardown) or `MAX_FAILURES` consecutive never-subscribed
/// attempts.
fn supervisor(shared: &Arc<Shared>) {
    let ctx = Map::new();
    let mut failures = 0u32;
    let mut backoff = BACKOFF_MIN;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        // A `down` status only re-arms via `ensure` — but the
        // supervisor itself exits after MAX_FAILURES, so reaching
        // here means we're retrying.
        match attempt(shared, &ctx) {
            // Subscribed then the stream ended — healthy feed that
            // dropped: reconnect on the minimum backoff.
            Ok(()) => {
                failures = 0;
                backoff = BACKOFF_MIN;
            }
            // Never subscribed — count toward give-up.
            Err(e) => {
                failures += 1;
                if let Ok(mut b) = shared.buffer.write() {
                    b.failures = failures;
                    if failures >= MAX_FAILURES {
                        b.set_phase(Phase::Down, Some(e));
                    } else {
                        b.set_phase(Phase::Stale, Some(e));
                    }
                }
                if failures >= MAX_FAILURES {
                    break;
                }
            }
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
    // Exited (give-up or stop) — mark down unless a newer attempt
    // already moved the phase.
    if let Ok(mut b) = shared.buffer.write()
        && b.phase != Phase::Live
    {
        let detail = b.detail.clone();
        b.set_phase(Phase::Down, detail);
    }
}

/// One feed attempt: spawn → `events.subscribe` → read `node.event`
/// notifications into the buffer until the stream dies. `Ok` means
/// the subscribe acked (the feed was live); `Err` carries the
/// failure — spawn error, subscribe error, or transport death
/// before/without a successful ack... a post-ack stream death is
/// still `Err` with the phase already `Live`→`Stale` set inside.
fn attempt(shared: &Arc<Shared>, ctx: &Map<String, Value>) -> Result<(), String> {
    let target = data::resolve_target(ctx);
    let data::ServeProc {
        child,
        mut stdin,
        mut stdout,
    } = data::spawn_serve(&target)?;
    let request = rpc::Request::new(SUBSCRIBE_ID, "events.subscribe", json!({}));
    let line = rpc::encode_line(&request).map_err(|e| e.to_string())?;
    writeln!(stdin, "{line}")
        .and_then(|()| stdin.flush())
        .map_err(|e| format!("sepia feed stdin: {e}"))?;
    // Park the child — the reader loop owns stdout; attempt end
    // (any exit path) drops FeedProc → kill+wait.
    if let Ok(mut slot) = shared.proc.lock() {
        *slot = Some(FeedProc { child, stdin });
    }

    let mut subscribed = false;
    let result = loop {
        let mut buf = String::new();
        match stdout.read_line(&mut buf) {
            Ok(0) => break Err("sepia serve feed closed stdout".to_string()),
            Ok(_) => {}
            Err(e) => break Err(format!("sepia serve feed stdout: {e}")),
        }
        match rpc::decode_line(buf.trim()) {
            Ok(Inbound::Response(resp)) if resp.id == SUBSCRIBE_ID => {
                if let Some(err) = resp.error {
                    break Err(format!("events.subscribe: {} ({})", err.message, err.code));
                }
                if let Ok(mut b) = shared.buffer.write() {
                    b.last_line = Some(SystemTime::now());
                    if let Some(live) = resp
                        .result
                        .as_ref()
                        .and_then(|r| r.get("live"))
                        .and_then(Value::as_array)
                    {
                        b.mark_live(
                            &live
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<_>>(),
                        );
                    }
                    b.set_phase(Phase::Live, None);
                    b.failures = 0;
                }
                subscribed = true;
            }
            Ok(Inbound::Notification(n)) if n.method == "node.event" => {
                if let Ok(mut b) = shared.buffer.write() {
                    b.ingest(&n.params);
                }
            }
            // Other notifications/responses/undecodable lines — the
            // feed child only ever sees our one subscribe, so these
            // are noise; skip like the data client does.
            _ => {}
        }
    };
    // Reap the child before reporting — FeedProc::drop kills it.
    if let Ok(mut slot) = shared.proc.lock() {
        slot.take();
    }
    let detail = result.as_ref().err().cloned();
    if subscribed {
        // The feed was live and dropped — stale until the next
        // attempt acks; the supervisor treats this as a clean
        // reconnect (failures reset).
        if let Ok(mut b) = shared.buffer.write() {
            b.set_phase(Phase::Stale, detail);
        }
        return Ok(());
    }
    if let Ok(mut b) = shared.buffer.write() {
        b.set_phase(Phase::Stale, detail);
    }
    result
}

// --------------------------------------------------------------- queries

/// Ensure the feed is armed, then snapshot the buffer.
fn with_buffer<R>(f: impl FnOnce(&Buffer) -> R) -> R {
    ensure();
    let Some(shared) = shared() else {
        // ensure() always leaves a Ready slot — unreachable, but
        // degrade honestly rather than panic on a poisoned mutex.
        return f(&Buffer::default());
    };
    match shared.buffer.read() {
        Ok(b) => f(&b),
        Err(_) => f(&Buffer::default()),
    }
}

/// `live` query rows — see [`live_rows`].
pub fn live() -> Vec<Value> {
    with_buffer(live_rows)
}

/// `permissions` query rows — see [`permission_rows`].
pub fn permissions() -> Vec<Value> {
    with_buffer(permission_rows)
}

/// `feed.status` query payload — see [`status_value`].
pub fn status() -> Value {
    with_buffer(status_value)
}

/// `stream` query tail — flattened buffered frames for `id`.
pub fn stream_tail(id: &str) -> Vec<Value> {
    with_buffer(|b| {
        b.sessions.get(id).map_or_else(Vec::new, |s| {
            flatten_events(&s.events.iter().cloned().collect::<Vec<_>>())
        })
    })
}

/// `(id, agent)` for mid-turn sessions — the `jobs` feed.
pub fn busy() -> Vec<(String, String)> {
    with_buffer(busy_sessions)
}

/// Mark a permission request resolved — called by `ops` after a
/// successful `sessions.permission` wire call. Returns whether the
/// ask was actually pending (mostly for tests).
pub fn resolve_permission(session_id: &str, request_id: &str) -> bool {
    let Some(shared) = shared() else {
        return false;
    };
    shared
        .buffer
        .write()
        .ok()
        .and_then(|mut b| {
            b.sessions
                .get_mut(session_id)
                .and_then(|s| s.pending.remove(request_id))
        })
        .is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn session_events(sid: &str, events: &[Value]) -> Value {
        json!({"type": "sessionEvents", "sessionId": sid, "events": events})
    }

    #[test]
    fn lifecycle_marks_attach_and_detach() {
        let mut b = Buffer::default();
        b.ingest(&json!({"type":"attached","sessionId":"s1","agentId":"devin"}));
        b.ingest(&json!({"type":"created","sessionId":"s2","agentId":"claude","cwd":"/w"}));
        let rows = live_rows(&b);
        assert_eq!(rows.len(), 2);
        let s1 = rows.iter().find(|r| r["id"] == "s1").unwrap();
        assert_eq!(s1["status"], "attached");
        assert_eq!(s1["agent"], "devin");
        b.ingest(
            &json!({"type":"detached","sessionId":"s1","agentId":"devin","reason":"explicit"}),
        );
        let s1 = live_rows(&b).into_iter().find(|r| r["id"] == "s1").unwrap();
        assert_eq!(s1["status"], "detached");
    }

    #[test]
    fn run_boundaries_flip_busy_and_clear_permissions() {
        let mut b = Buffer::default();
        b.ingest(&json!({"type":"attached","sessionId":"s1","agentId":"devin"}));
        b.ingest(&session_events(
            "s1",
            &[
                json!({"type":"runStarted","threadId":"t","runId":"r"}),
                json!({"type":"custom","name":"acp:permission_request","value":{
                    "requestId":"req1","sessionId":"s1","title":"Run rm -rf",
                    "toolKind":"execute","options":[{"optionId":"allow","name":"Allow","kind":"allow_once"}]
                }}),
            ],
        ));
        assert_eq!(b.sessions["s1"].pending.len(), 1);
        assert!(b.sessions["s1"].running);
        assert_eq!(permission_rows(&b).len(), 1);
        // resolve path
        assert!(
            b.sessions
                .get_mut("s1")
                .unwrap()
                .pending
                .remove("req1")
                .is_some()
        );
        assert_eq!(permission_rows(&b).len(), 0);
        // runFinished clears pending + busy
        b.ingest(&session_events(
            "s1",
            &[
                json!({"type":"custom","name":"acp:permission_request","value":{"requestId":"req2","title":"x","options":[]}}),
                json!({"type":"runFinished","threadId":"t","runId":"r"}),
            ],
        ));
        assert!(!b.sessions["s1"].running);
        assert_eq!(b.sessions["s1"].pending.len(), 0);
    }

    #[test]
    fn detach_and_delete_clear_pending() {
        let mut b = Buffer::default();
        b.ingest(&session_events(
            "s1",
            &[json!({"type":"custom","name":"acp:permission_request","value":{"requestId":"r","title":"t","options":[]}})],
        ));
        assert_eq!(b.sessions["s1"].pending.len(), 1);
        b.ingest(&json!({"type":"detached","sessionId":"s1","agentId":"a"}));
        assert_eq!(b.sessions["s1"].pending.len(), 0);
    }

    #[test]
    fn lagged_is_counted_never_dropped() {
        let mut b = Buffer::default();
        b.ingest(&json!({"type":"lagged","sessionId":"s1","count":7}));
        b.ingest(&json!({"type":"lagged","sessionId":null,"count":3}));
        assert_eq!(b.lagged_total, 10);
        assert_eq!(b.sessions["s1"].lagged, 7);
        assert_eq!(status_value(&b)["lagged"], 10);
    }

    #[test]
    fn event_caps_drop_oldest() {
        let mut b = Buffer::default();
        let events: Vec<Value> = (0..EVENTS_PER_SESSION + 50)
            .map(|i| json!({"type":"textMessageContent","messageId":"m","delta":format!("d{i}")}))
            .collect();
        b.ingest(&session_events("s1", &events));
        let s = &b.sessions["s1"];
        assert_eq!(s.events.len(), EVENTS_PER_SESSION);
        // oldest dropped — first retained frame is d50
        let first = &s.events.front().unwrap().1;
        assert_eq!(first["delta"], "d50");
    }

    #[test]
    fn permission_request_extracts_wire_fields() {
        let ev = json!({"type":"custom","name":"acp:permission_request","value":{
            "requestId":"r9","sessionId":"s1","toolCallId":"tc1","toolKind":"edit",
            "title":"Write file","options":[{"optionId":"o1","name":"Once","kind":"allow_once"}]
        }});
        let p = permission_request(&ev).unwrap();
        assert_eq!(p.request_id, "r9");
        assert_eq!(p.tool, "edit"); // toolKind wins over toolCallId
        assert_eq!(p.options.len(), 1);
        assert!(permission_request(&json!({"type":"custom","name":"other","value":{}})).is_none());
    }

    #[test]
    fn flatten_events_builds_transcript_rows() {
        let ts = SystemTime::now();
        let ev = |v: Value| (ts, v, 0usize);
        let events = vec![
            ev(json!({"type":"runStarted","threadId":"t","runId":"r"})),
            ev(json!({"type":"textMessageStart","messageId":"m1","role":"assistant"})),
            ev(json!({"type":"textMessageContent","messageId":"m1","delta":"Hello "})),
            ev(json!({"type":"textMessageContent","messageId":"m1","delta":"world"})),
            ev(json!({"type":"textMessageEnd","messageId":"m1"})),
            ev(json!({"type":"toolCallStart","toolCallId":"tc","toolCallName":"bash"})),
            ev(json!({"type":"toolCallArgs","toolCallId":"tc","delta":"{\"cmd\":"})),
            ev(json!({"type":"toolCallEnd","toolCallId":"tc","status":"done"})),
            ev(
                json!({"type":"custom","name":"acp:permission_request","value":{"requestId":"r","title":"Run it","options":[{"optionId":"allow"}]}}),
            ),
            ev(json!({"type":"runFinished","threadId":"t","runId":"r"})),
        ];
        let rows = flatten_events(&events);
        let roles: Vec<&str> = rows.iter().filter_map(|r| r["role"].as_str()).collect();
        assert_eq!(
            roles,
            ["run", "assistant", "tool", "permission", "run"],
            "{rows:?}"
        );
        assert_eq!(rows[1]["content"], "Hello world");
        assert_eq!(rows[2]["tool"], "bash");
        assert_eq!(rows[2]["content"], "{\"cmd\":");
        assert_eq!(rows[3]["role"], "permission");
        assert!(rows[3]["content"].as_str().unwrap().contains("Run it"));
    }

    #[test]
    fn flatten_flushes_open_frames_at_tail() {
        let ts = SystemTime::now();
        let events = vec![
            (
                ts,
                json!({"type":"textMessageStart","messageId":"m","role":"assistant"}),
                0,
            ),
            (
                ts,
                json!({"type":"textMessageContent","messageId":"m","delta":"partial"}),
                0,
            ),
        ];
        let rows = flatten_events(&events);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["content"], "partial");
    }

    #[test]
    fn permission_rows_hide_stale_asks() {
        let mut b = Buffer::default();
        let mut s = SessionFeed::default();
        s.pending.insert(
            "old".into(),
            PendingPermission {
                request_id: "old".into(),
                title: "t".into(),
                tool: String::new(),
                options: vec![],
                since: SystemTime::now() - PERMISSION_TTL - Duration::from_secs(1),
            },
        );
        s.pending.insert(
            "new".into(),
            PendingPermission {
                request_id: "new".into(),
                title: "t".into(),
                tool: String::new(),
                options: vec![],
                since: SystemTime::now(),
            },
        );
        b.sessions.insert("s1".into(), s);
        let rows = permission_rows(&b);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["request"], "new");
    }

    #[test]
    fn session_cap_evicts_stalest() {
        let mut b = Buffer::default();
        for i in 0..MAX_SESSIONS {
            b.ingest(&session_events(
                &format!("s{i}"),
                &[json!({"type":"runStarted"})],
            ));
        }
        assert_eq!(b.sessions.len(), MAX_SESSIONS);
        // a brand-new session evicts the oldest (s0 — all share ~now,
        // min_by_key picks an arbitrary oldest; count stays capped)
        b.ingest(&session_events("new", &[json!({"type":"runStarted"})]));
        assert_eq!(b.sessions.len(), MAX_SESSIONS);
        assert!(b.sessions.contains_key("new"));
    }
}
