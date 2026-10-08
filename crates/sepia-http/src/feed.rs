//! The node event feed behind `GET /api/events`, the meta-store
//! instrumentation that writes to it, the held-session watch, and the
//! per-session live listener that turns run lifecycle into `busy` feed
//! events — ports of `apps/server/src/events.ts` and the watch/listener
//! halves of `app.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sepia_control::{ControlPlane, SessionSummary};
use sepia_meta::{MetaStore, RunSpan, SessionMeta};
use sepia_proto::SessionEvent;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Bound per subscriber — a client this far behind is resyncing anyway.
const MAX_QUEUED: usize = 500;

/// A held session stops being watched this long after the last read-only
/// attach observed it — a closed tab can't leave the probe running forever.
const HELD_WATCH_TTL: Duration = Duration::from_secs(30 * 60);

/// `{"id","agent"?,"patch"}` — the wire shape of session/meta events.
pub fn session_payload(id: &str, agent: Option<&str>, patch: Value) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), Value::String(id.to_string()));
    if let Some(agent) = agent {
        payload.insert("agent".into(), Value::String(agent.to_string()));
    }
    payload.insert("patch".into(), patch);
    Value::Object(payload)
}

/// One node-feed event: `kind` is `session` | `meta` | `project`.
#[derive(Clone, Debug)]
pub struct NodeEvent {
    pub kind: &'static str,
    pub payload: Value,
}

/// Process-wide emitter the routes write and SSE connections drain —
/// `tokio::sync::broadcast` already implements the TS feed's semantics
/// (bounded per-subscriber queue, oldest dropped under lag).
#[derive(Clone)]
pub struct EventFeed {
    tx: broadcast::Sender<NodeEvent>,
}

impl Default for EventFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl EventFeed {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(MAX_QUEUED);
        Self { tx }
    }

    pub fn emit(&self, kind: &'static str, payload: Value) {
        // No active receivers is not an error.
        let _ = self.tx.send(NodeEvent { kind, payload });
    }

    /// Emit the `{"id","agent","patch"}` session-row invalidation.
    pub fn emit_session(&self, id: &str, agent: Option<&str>, patch: Value) {
        self.emit("session", session_payload(id, agent, patch));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.tx.subscribe()
    }

    /// Live SSE subscribers — background probes (the held-session watch)
    /// gate on someone listening.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

/// The session row's `busy` flag mirrors the live turn lifecycle.
pub fn busy_from_events(events: &[SessionEvent]) -> Option<bool> {
    // Both edges can land in one batch (load-failure unwind) — the run
    // is over.
    if events
        .iter()
        .any(|e| matches!(e, SessionEvent::RunFinished { .. }))
    {
        return Some(false);
    }
    if events
        .iter()
        .any(|e| matches!(e, SessionEvent::RunStarted { .. }))
    {
        return Some(true);
    }
    None
}

/// `MetaStore` + feed emission — `instrumentMeta` in TS: every overlay
/// write also lands on the node feed (`meta`/`project` events).
#[derive(Clone)]
pub struct InstrumentedMeta {
    store: MetaStore,
    feed: EventFeed,
}

impl InstrumentedMeta {
    pub fn new(store: MetaStore, feed: EventFeed) -> Self {
        Self { store, feed }
    }

    pub fn store(&self) -> &MetaStore {
        &self.store
    }

    pub fn of(&self, id: &str) -> Option<SessionMeta> {
        self.store.of(id)
    }

    pub fn sessions(&self) -> std::collections::BTreeMap<String, SessionMeta> {
        self.store.sessions()
    }

    /// Merge a patch into one session's overlay; emits a `meta` event
    /// carrying the applied fields.
    pub fn patch(&self, id: &str, patch: &SessionMeta) {
        self.store.patch(id, patch);
        let agent = self.store.of(id).and_then(|m| m.agent);
        let patch_json = serde_json::to_value(patch).unwrap_or_else(|_| json!({}));
        self.feed
            .emit("meta", session_payload(id, agent.as_deref(), patch_json));
    }

    /// Record a run span; emits only when the append wasn't a no-op.
    pub fn add_span(&self, id: &str, span: RunSpan) {
        let before = self.store.of(id).and_then(|m| m.spans);
        self.store.add_span(id, span);
        let after = self.store.of(id).and_then(|m| m.spans);
        if after != before {
            let agent = self.store.of(id).and_then(|m| m.agent);
            self.feed.emit(
                "meta",
                session_payload(
                    id,
                    agent.as_deref(),
                    json!({ "spans": after.unwrap_or_default() }),
                ),
            );
        }
    }

    pub fn remove(&self, id: &str) {
        let existing = self.store.of(id);
        self.store.remove(id);
        if let Some(meta) = existing {
            self.feed.emit(
                "meta",
                session_payload(id, meta.agent.as_deref(), json!({ "deleted": true })),
            );
        }
    }

    pub fn list_projects(&self) -> Vec<sepia_meta::Project> {
        self.store.list_projects()
    }

    pub fn create_project(&self, name: &str) -> sepia_meta::Project {
        let project = self.store.create_project(name);
        self.feed.emit(
            "project",
            json!({ "id": project.id, "patch": { "name": project.name } }),
        );
        project
    }

    /// Create under a caller-chosen id or refresh the name — the
    /// idempotent half of a project transfer.
    pub fn ensure_project(&self, id: &str, name: &str) -> sepia_meta::Project {
        let before = self
            .store
            .list_projects()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name);
        let project = self.store.ensure_project(id, name);
        if before.as_deref() != Some(name) {
            self.feed
                .emit("project", json!({ "id": id, "patch": { "name": name } }));
        }
        project
    }

    pub fn rename_project(&self, id: &str, name: &str) -> bool {
        let renamed = self.store.rename_project(id, name);
        if renamed {
            self.feed
                .emit("project", json!({ "id": id, "patch": { "name": name } }));
        }
        renamed
    }

    pub fn delete_project(&self, id: &str) {
        let existed = self.store.list_projects().iter().any(|p| p.id == id);
        self.store.delete_project(id);
        if existed {
            self.feed
                .emit("project", json!({ "id": id, "patch": { "deleted": true } }));
        }
    }

    pub fn config(&self) -> std::collections::BTreeMap<String, Value> {
        self.store.config()
    }

    /// Config isn't part of the feed — passes through untouched.
    pub fn set_config(&self, key: &str, value: Value) {
        self.store.set_config(key, value);
    }
}

/// One watched held session: a read-only attach means another process
/// owns the store lock — the holder's transcript writes and its eventual
/// release happen outside this node, so the watch re-lists with locks
/// and diffs each watched summary into `session` feed events.
struct HeldWatch {
    id: String,
    /// Owning agent when the attach resolved one — disambiguates
    /// colliding ids.
    agent: Option<String>,
    /// Last attach that saw the session held; refreshes `HELD_WATCH_TTL`.
    at: Instant,
    /// Diff baseline — seeded `{locked: true}` so the first tick reports
    /// either the holder details or the release edge.
    baseline: serde_json::Map<String, Value>,
}

/// The fields a held tick diffs — title/updatedAt/locked/lockHolderPid/
/// busy, serialized under their wire names.
const HELD_FIELDS: [&str; 5] = ["title", "updatedAt", "locked", "lockHolderPid", "busy"];

fn held_snapshot(summary: &SessionSummary) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("title".into(), Value::String(summary.title.clone()));
    map.insert(
        "updatedAt".into(),
        Value::String(summary.updated_at.clone()),
    );
    map.insert("locked".into(), Value::Bool(summary.locked));
    map.insert(
        "lockHolderPid".into(),
        summary.lock_holder_pid.map_or(Value::Null, Value::from),
    );
    map.insert("busy".into(), Value::Bool(summary.busy));
    map
}

/// Held-watch state shared between the attach route (registers) and the
/// ticker task (probes). `None` interval disables the watch entirely.
#[derive(Clone)]
pub struct HeldWatches {
    inner: Arc<Mutex<HashMap<String, HeldWatch>>>,
    interval: Option<Duration>,
}

impl HeldWatches {
    pub fn new(interval: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            interval: (interval > Duration::ZERO).then_some(interval),
        }
    }

    /// Mark a session held — keys carry the agent because session ids
    /// collide across agent stores.
    pub fn watch(&self, id: &str, agent: Option<&str>) {
        let key = format!("{}:{id}", agent.unwrap_or_default());
        let mut baseline = serde_json::Map::new();
        baseline.insert("locked".into(), Value::Bool(true));
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                key,
                HeldWatch {
                    id: id.to_string(),
                    agent: agent.map(str::to_string),
                    at: Instant::now(),
                    baseline,
                },
            );
    }

    /// Stop watching `id` (all agents when `agent` is `None`).
    pub fn unwatch(&self, id: &str, agent: Option<&str>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, w| !(w.id == id && agent.is_none_or(|a| w.agent.as_deref() == Some(a))));
    }

    /// One probe round — the ticker body, also directly testable.
    pub async fn tick(&self, plane: &ControlPlane, feed: &EventFeed) {
        {
            let now = Instant::now();
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|_, w| now.duration_since(w.at) <= HELD_WATCH_TTL);
        }
        if self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return;
        }
        // Nobody drains the feed — the probe (agent spawns) buys nothing.
        if feed.subscriber_count() == 0 {
            return;
        }
        let listed = match plane.list_sessions(true).await {
            Ok(list) => list,
            Err(error) => {
                tracing::warn!("held-session watch failed: {error}");
                return;
            }
        };
        let mut emitted = Vec::new();
        {
            // `retain` passes `&mut` values — enough to swap baselines.
            let mut watches = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            watches.retain(|_, watch| {
                let summary = listed.iter().find(|row| {
                    row.id == watch.id && watch.agent.as_deref().is_none_or(|a| row.agent == a)
                });
                let Some(summary) = summary else {
                    // The store row vanished while held — same shape as
                    // DELETE.
                    emitted.push(session_payload(
                        &watch.id,
                        watch.agent.as_deref(),
                        json!({ "deleted": true }),
                    ));
                    return false;
                };
                let current = held_snapshot(summary);
                let mut patch = serde_json::Map::new();
                for field in HELD_FIELDS {
                    if watch.baseline.get(field) != current.get(field) {
                        patch.insert(
                            field.to_string(),
                            current.get(field).cloned().unwrap_or(Value::Null),
                        );
                    }
                }
                watch.baseline = current;
                if !patch.is_empty() {
                    emitted.push(session_payload(
                        &summary.id,
                        Some(summary.agent.as_str()),
                        Value::Object(patch),
                    ));
                }
                true
            });
        }
        for payload in emitted {
            feed.emit("session", payload);
        }
    }

    /// Spawn the ticker — re-probes every `interval` while watches live.
    /// Called once at app build; a `0` interval disables the watch.
    pub fn spawn_ticker(&self, plane: Arc<ControlPlane>, feed: EventFeed) {
        let Some(interval) = self.interval else {
            return;
        };
        let watches = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                watches.tick(&plane, &feed).await;
            }
        });
    }
}

/// Per-session live listeners — the app-level `registerLiveListener`
/// half that emits `busy` transitions onto the feed even when no client
/// has the session open. Keyed by bare session id like the TS map.
#[derive(Clone, Default)]
pub struct LiveListeners {
    inner: Arc<Mutex<HashSet<String>>>,
}

impl LiveListeners {
    /// Subscribe a forward task for `id` that mirrors run lifecycle onto
    /// the feed as `busy` patches AND fires push notifications — a no-op
    /// when a listener is already registered (matching the TS
    /// `liveUnsubs` dedupe semantics: the entry stays even after the
    /// task ends).
    pub fn register(
        &self,
        plane: &Arc<ControlPlane>,
        feed: &EventFeed,
        push: Option<Arc<sepia_push::PushStore>>,
        id: &str,
        agent_id: Option<String>,
    ) {
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !inner.insert(id.to_string()) {
                return;
            }
        }
        let plane = Arc::clone(plane);
        let feed = feed.clone();
        let id_owned = id.to_string();
        tokio::spawn(async move {
            // The subscription is resolved inside the task so `register`
            // stays sync like the TS call site.
            let Ok(mut rx) = plane.subscribe(&id_owned, agent_id.as_deref()).await else {
                return;
            };
            loop {
                match rx.recv().await {
                    Ok(events) => {
                        if let Some(push) = &push {
                            let title = plane
                                .get_summary(&id_owned, None)
                                .await
                                .map_or_else(|_| id_owned.clone(), |s| s.title);
                            sepia_push::notify_for_events(
                                push,
                                &id_owned,
                                agent_id.as_deref(),
                                &title,
                                &events,
                            );
                        }
                        if let Some(busy) = busy_from_events(&events) {
                            feed.emit_session(
                                &id_owned,
                                agent_id.as_deref(),
                                json!({ "busy": busy }),
                            );
                        }
                    }
                    // A lagged listener just missed an edge; the next one
                    // still flips the flag.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }
}
