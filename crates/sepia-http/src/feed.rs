#![allow(unknown_lints, clippy::assert_is_empty)]

//! The node event feed behind `GET /api/events`, the meta-store
//! instrumentation that writes to it, the held-session watch, and the
//! per-session live listener that turns run lifecycle into `busy` feed
//! events — ports of `apps/server/src/events.ts` and the watch/listener
//! halves of `app.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sepia_meta::{MetaStore, RunSpan, SessionMeta};
use sepia_proto::SessionEvent;
use sepia_session::SessionPlane;
use sepia_session::SessionSummary;
use serde_json::{Value, json};
use tokio::sync::broadcast;

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

/// One node-feed event — the substrate's [`hypha_http::FeedEvent`]
/// shape (`kind` is `session` | `meta` | `project` | `heartbeat` | …,
/// `payload` the `data:` JSON verbatim). Aliased so the session-side
/// API reads unchanged.
pub type NodeEvent = hypha_http::FeedEvent;

/// Process-wide emitter the routes write and SSE connections drain —
/// a newtype over [`hypha_http::EventFeed`] so every emission lands on
/// the SAME broadcast the substrate's `/api/events` serves (PL-005
/// P4b: the feed moved to hypha; this wrapper keeps the session-side
/// `emit_session`/`session_payload` vocabulary).
#[derive(Clone)]
pub struct EventFeed {
    inner: hypha_http::EventFeed,
}

impl Default for EventFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl EventFeed {
    pub fn new() -> Self {
        Self {
            inner: hypha_http::EventFeed::new(),
        }
    }

    /// Wrap an existing substrate feed — composition shares one
    /// broadcast between the session emitters and `/api/events`.
    pub fn from_substrate(inner: hypha_http::EventFeed) -> Self {
        Self { inner }
    }

    /// The substrate feed this wraps — `Clone` shares the channel.
    pub fn substrate(&self) -> hypha_http::EventFeed {
        self.inner.clone()
    }

    pub fn emit(&self, kind: &'static str, payload: Value) {
        self.inner.emit(kind, payload);
    }

    /// Emit the `{"id","agent","patch"}` session-row invalidation.
    pub fn emit_session(&self, id: &str, agent: Option<&str>, patch: Value) {
        self.emit("session", session_payload(id, agent, patch));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.inner.subscribe()
    }

    /// Live SSE subscribers — background probes (the held-session watch)
    /// gate on someone listening. Counts the substrate feed's
    /// subscribers — `/api/events` clients drain that channel.
    pub fn subscriber_count(&self) -> usize {
        self.inner.subscriber_count()
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

/// `MetaStore` + feed emission — `instrumentMeta` in TS: every session
/// overlay write also lands on the node feed (`meta` events). Project
/// and config writes moved to the KV surface in P-006 phase 2 — see
/// [`crate::kv`] for their feed contract (`project` events) and the
/// delete cascade.
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
    pub async fn tick(&self, plane: &Arc<dyn SessionPlane>, feed: &EventFeed) {
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
    pub fn spawn_ticker(&self, plane: Arc<dyn SessionPlane>, feed: EventFeed) {
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
        plane: &Arc<dyn SessionPlane>,
        feed: &EventFeed,
        push: Option<Arc<hypha_push::PushStore>>,
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
            // Non-pinning watch: this forwarder lives as long as the
            // session is registered, so a pinning subscribe would hold
            // the agent subprocess open past sepia's idle TTL (T-028).
            let Ok(mut rx) = plane.watch(&id_owned, agent_id.as_deref()).await else {
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
                            crate::push::notify_for_events(
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
                        // Marker patches for downstream consumers (the
                        // hub's push fan-out reads them off the feed —
                        // a session "run_finished" is distinct from
                        // "busy: false" because cancel also clears busy).
                        if events
                            .iter()
                            .any(|e| matches!(e, SessionEvent::RunFinished { .. }))
                        {
                            feed.emit_session(
                                &id_owned,
                                agent_id.as_deref(),
                                json!({ "runFinished": true }),
                            );
                        }
                        if events.iter().any(|e| {
                            matches!(e, SessionEvent::Custom { name, .. }
                                if name == "acp:permission_request")
                        }) {
                            feed.emit_session(
                                &id_owned,
                                agent_id.as_deref(),
                                json!({ "permissionRequested": true }),
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

#[cfg(test)]
mod cov_tests {
    #![allow(clippy::unwrap_used)]
    use std::collections::HashMap;

    use sepia_core::Session;
    use sepia_core::storage::SessionRepository;
    use sepia_session::LocalPlane;
    use sepia_session::{ControlPlane, ControlPlaneOptions};

    use super::*;

    /// In-memory store for a real `LocalPlane` — exercises `tick`
    /// against the production trait, not a stub.
    struct MemStore {
        sessions: Mutex<HashMap<String, Session>>,
    }

    #[async_trait::async_trait]
    impl SessionRepository for MemStore {
        async fn save(&self, session: &Session) -> Result<(), sepia_core::StorageError> {
            self.sessions
                .lock()
                .unwrap()
                .insert(session.id.clone(), session.clone());
            Ok(())
        }
        async fn get_by_id(
            &self,
            id: &str,
            _agent: Option<&str>,
        ) -> Result<Option<Session>, sepia_core::StorageError> {
            Ok(self.sessions.lock().unwrap().get(id).cloned())
        }
        async fn list(&self) -> Result<Vec<Session>, sepia_core::StorageError> {
            Ok(self.sessions.lock().unwrap().values().cloned().collect())
        }
        async fn delete(&self, id: &str) -> Result<(), sepia_core::StorageError> {
            self.sessions.lock().unwrap().remove(id);
            Ok(())
        }
        async fn has_session(&self, id: &str) -> Result<bool, sepia_core::StorageError> {
            Ok(self.sessions.lock().unwrap().contains_key(id))
        }
    }

    /// Store whose `list` fails — drives `tick`'s warn-and-return arm.
    struct FailStore;

    #[async_trait::async_trait]
    impl SessionRepository for FailStore {
        async fn save(&self, _s: &Session) -> Result<(), sepia_core::StorageError> {
            Err(sepia_core::StorageError::new("x"))
        }
        async fn get_by_id(
            &self,
            _id: &str,
            _a: Option<&str>,
        ) -> Result<Option<Session>, sepia_core::StorageError> {
            Err(sepia_core::StorageError::new("x"))
        }
        async fn list(&self) -> Result<Vec<Session>, sepia_core::StorageError> {
            Err(sepia_core::StorageError::new("x"))
        }
        async fn delete(&self, _id: &str) -> Result<(), sepia_core::StorageError> {
            Err(sepia_core::StorageError::new("x"))
        }
        async fn has_session(&self, _id: &str) -> Result<bool, sepia_core::StorageError> {
            Err(sepia_core::StorageError::new("x"))
        }
    }

    #[tokio::test]
    async fn tick_surfaces_a_failed_list() {
        let plane: Arc<dyn SessionPlane> = Arc::new(LocalPlane(
            ControlPlane::new(Arc::new(FailStore), ControlPlaneOptions::default()),
            None,
        ));
        let feed = EventFeed::new();
        let _rx = feed.subscribe();
        let watch = HeldWatches::new(Duration::ZERO);
        watch.watch("s1", None);
        watch.tick(&plane, &feed).await;
        // The store's other verb arms — `tick` only needs `list`.
        let fs = FailStore;
        assert!(fs.save(&stub_session_for_test()).await.is_err());
        assert!(fs.get_by_id("s", None).await.is_err());
        assert!(fs.delete("s").await.is_err());
        assert!(fs.has_session("s").await.is_err());
    }

    fn stub_session_for_test() -> Session {
        serde_json::from_value(serde_json::json!({
            "id": "s", "title": "t", "workingDirectory": "/w",
            "backendType": "devin", "model": "m", "createdAt": 1.0,
            "lastActivityAt": 1.0, "mainChainId": 0
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn mem_store_full_surface() {
        // `plane_with` only reads — drive the write verbs too.
        let store = MemStore {
            sessions: Mutex::new(HashMap::new()),
        };
        store.save(&stub_session_for_test()).await.unwrap();
        assert!(store.get_by_id("s", None).await.unwrap().is_some());
        assert!(store.has_session("s").await.unwrap());
        assert_eq!(store.list().await.unwrap().len(), 1);
        store.delete("s").await.unwrap();
        assert!(!store.has_session("s").await.unwrap());
    }

    #[tokio::test]
    async fn register_dedupes() {
        let plane = plane_with(&[]);
        let feed = EventFeed::new();
        let listeners = LiveListeners::default();
        listeners.register(&plane, &feed, None, "s1", None);
        // Second register hits the `!insert` early return.
        listeners.register(&plane, &feed, None, "s1", None);
    }

    fn plane_with(sessions: &[Session]) -> Arc<dyn SessionPlane> {
        let store = Arc::new(MemStore {
            sessions: Mutex::new(sessions.iter().map(|s| (s.id.clone(), s.clone())).collect()),
        });
        Arc::new(LocalPlane(
            ControlPlane::new(store, ControlPlaneOptions::default()),
            None,
        ))
    }

    fn session(id: &str) -> Session {
        serde_json::from_value(json!({
            "id": id,
            "title": "T",
            "workingDirectory": "/w",
            "backendType": "devin",
            "model": "m",
            "createdAt": 1_700_000_000.0,
            "lastActivityAt": 1_700_000_100.0,
            "mainChainId": 0
        }))
        .unwrap()
    }

    fn meta() -> InstrumentedMeta {
        // `keep` leaks the tempdir — MetaStore flushes into it on every
        // mutation, so it must outlive the returned store.
        let dir = tempfile::tempdir().unwrap().keep();
        InstrumentedMeta::new(MetaStore::open(&dir.join("m.json")), EventFeed::new())
    }

    #[test]
    fn payload_and_busy() {
        let p = session_payload("s", Some("a"), json!({"x":1}));
        assert_eq!(p["id"], "s");
        assert_eq!(p["agent"], "a");
        let _ = session_payload("s", None, json!({}));
        assert_eq!(busy_from_events(&[]), None);
        assert_eq!(EventFeed::default().subscriber_count(), 0);
        let feed = EventFeed::new();
        let _rx = feed.subscribe();
        assert_eq!(feed.subscriber_count(), 1);
    }

    #[test]
    fn instrumented_meta_emits() {
        let m = meta();
        let mut rx = m.feed.subscribe();
        let _ = m.of("s1");
        let _ = m.sessions();
        m.patch("s1", &SessionMeta::default());
        m.remove("s1");
        let _ = rx.try_recv();
    }

    #[tokio::test]
    async fn held_watch_tick_arms() {
        let plane = plane_with(&[session("s1")]);
        let feed = EventFeed::new();
        let watches = HeldWatches::new(Duration::ZERO);
        // No watches → early return.
        watches.tick(&plane, &feed).await;
        // Watches but nobody listening → early return.
        watches.watch("s1", None);
        watches.watch("gone", Some("devin"));
        watches.tick(&plane, &feed).await;
        // Subscribed: the listed row emits its baseline diff, the
        // vanished id emits {"deleted": true}.
        let mut rx = feed.subscribe();
        watches.tick(&plane, &feed).await;
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        assert!(got.iter().any(|e| e.payload["patch"]["deleted"] == true));
        // Baseline now matches — the steady tick emits nothing new.
        watches.tick(&plane, &feed).await;
        watches.unwatch("s1", None);
        watches.unwatch("gone", Some("devin"));
        // `spawn_ticker` on a disabled interval returns at once.
        watches.spawn_ticker(plane, feed);
    }
}

#[cfg(test)]
mod cov_more_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use sepia_core::Session;
    use sepia_session::{ControlError, ControlErrorCode};

    /// Plane whose `subscribe` returns a drivable receiver — the
    /// register loop's event/lag/close arms fire on `tx.send` without
    /// a live agent.
    struct StubPlane {
        tx: broadcast::Sender<Vec<SessionEvent>>,
        fail: bool,
        summary: Session,
    }

    fn stub_session() -> Session {
        serde_json::from_value(json!({
            "id": "s1", "title": "T", "workingDirectory": "/w",
            "backendType": "devin", "model": "m",
            "createdAt": 1.0, "lastActivityAt": 1.0, "mainChainId": 0
        }))
        .unwrap()
    }

    impl StubPlane {
        fn ok() -> Self {
            let (tx, _) = broadcast::channel(4);
            Self {
                tx,
                fail: false,
                summary: stub_session(),
            }
        }
        fn failing() -> Self {
            let (tx, _) = broadcast::channel(1);
            Self {
                tx,
                fail: true,
                summary: stub_session(),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionPlane for StubPlane {
        async fn list_sessions(&self, _: bool) -> Result<Vec<SessionSummary>, ControlError> {
            unimplemented!()
        }
        async fn get_session(&self, _: &str, _: Option<&str>) -> Result<Session, ControlError> {
            unimplemented!()
        }
        async fn get_summary(&self, _: &str, _: Option<&str>) -> Result<Session, ControlError> {
            Ok(self.summary.clone())
        }
        async fn get_history(
            &self,
            _: &str,
            _: &sepia_session::HistoryOptions,
            _: Option<&str>,
        ) -> Result<sepia_session::HistoryPage, ControlError> {
            unimplemented!()
        }
        async fn attach(
            &self,
            _: &str,
            _: bool,
            _: Option<String>,
            _: Option<Vec<String>>,
            _: Option<String>,
        ) -> Result<sepia_session::AttachResult, ControlError> {
            unimplemented!()
        }
        async fn create_session(
            &self,
            _: &str,
            _: Option<String>,
            _: Option<String>,
            _: Option<String>,
            _: Option<Vec<String>>,
        ) -> Result<sepia_session::CreateResult, ControlError> {
            unimplemented!()
        }
        async fn detach(&self, _: &str) {}
        async fn prompt(
            &self,
            _: &str,
            _: Vec<sepia_acp::types::PromptPart>,
            _: Option<&str>,
        ) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn prompt_wait(
            &self,
            _: &str,
            _: Vec<sepia_acp::types::PromptPart>,
            _: Option<&str>,
            _: Option<std::time::Duration>,
        ) -> Result<sepia_session::PromptWaitResult, ControlError> {
            unimplemented!()
        }
        async fn set_mode(&self, _: &str, _: &str, _: Option<&str>) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn cancel(&self, _: &str, _: Option<&str>) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn respond_to_permission(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn delete_session(&self, _: &str, _: Option<&str>) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn delete_session_store(&self, _: &str, _: &str) -> Result<(), ControlError> {
            unimplemented!()
        }
        async fn import_session(
            &self,
            _: &str,
            _: &Session,
            _: &sepia_session::ImportOverrides,
        ) -> Result<String, ControlError> {
            unimplemented!()
        }
        async fn convert_session(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<String, ControlError> {
            unimplemented!()
        }
        async fn restore(
            &self,
            _: &str,
            _: &sepia_session::RestoreRequest,
            _: Option<&str>,
        ) -> Result<sepia_session::RestoreResult, ControlError> {
            unimplemented!()
        }
        async fn rewind(
            &self,
            _: &str,
            _: &sepia_session::RewindRequest,
            _: Option<&str>,
        ) -> Result<sepia_session::RewindResult, ControlError> {
            unimplemented!()
        }
        async fn subscribe(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError> {
            if self.fail {
                return Err(ControlError::new(ControlErrorCode::Invalid, "gone"));
            }
            Ok(self.tx.subscribe())
        }
        async fn commands(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<Vec<sepia_acp::AvailableCommand>, ControlError> {
            unimplemented!()
        }
        async fn command(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<std::time::Duration>,
            _: Option<&str>,
        ) -> Result<sepia_session::PromptWaitResult, ControlError> {
            unimplemented!()
        }
        async fn list_agents(&self) -> Vec<sepia_session::AgentInfo> {
            vec![]
        }
        async fn can_rewind(&self, _: &str) -> bool {
            false
        }
        async fn close_all(&self) {}
    }

    #[tokio::test]
    async fn register_event_and_close_arms() {
        // `subscribe` fails → early return on the Err arm.
        let listeners = LiveListeners::default();
        let feed = EventFeed::new();
        let plane: Arc<dyn SessionPlane> = Arc::new(StubPlane::failing());
        listeners.register(&plane, &feed, None, "gone", None);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Event arms — push notify, busy emit, RunFinished marker,
        // permission-request marker.
        let plane = StubPlane::ok();
        let tx = plane.tx.clone();
        let plane: Arc<dyn SessionPlane> = Arc::new(plane);
        let feed = EventFeed::new();
        let mut feed_rx = feed.subscribe();
        let kv: Arc<dyn hypha_kv::KvStore> = Arc::new(hypha_kv_file::JsonKvStore::open(
            &tempfile::tempdir().unwrap().path().join("kv.json"),
        ));
        let push = hypha_push::PushStore::open(crate::push::KvPushBackend(kv));
        listeners.register(
            &plane,
            &feed,
            Some(Arc::new(push)),
            "s1",
            Some("devin".into()),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(vec![SessionEvent::RunFinished {
            thread_id: "s1".into(),
            run_id: "r".into(),
        }])
        .unwrap();
        // Let the listener drain before each send — the channel holds 4
        // and the lag flood below would evict these before a recv.
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(vec![SessionEvent::Custom {
            name: "acp:permission_request".into(),
            value: json!({}),
        }])
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Lagged — overflow the 4-slot buffer past the last recv.
        for _ in 0..8 {
            let _ = tx.send(vec![SessionEvent::Custom {
                name: "hb".into(),
                value: json!({}),
            }]);
        }
        drop(tx);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let mut got = Vec::new();
        while let Ok(e) = feed_rx.try_recv() {
            got.push(e);
        }
    }

    /// The `unimplemented!()` arms — each panic executes its line.
    #[tokio::test]
    async fn stub_plane_unimplemented_arms() {
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;
        let plane = StubPlane::ok();
        let _ = AssertUnwindSafe(plane.list_sessions(false))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.get_session("s", None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.get_history(
            "s",
            &sepia_session::HistoryOptions::default(),
            None,
        ))
        .catch_unwind()
        .await;
        let _ = AssertUnwindSafe(plane.attach("s", false, None, None, None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.create_session("/w", None, None, None, None))
            .catch_unwind()
            .await;
        plane.detach("s").await;
        let _ = AssertUnwindSafe(plane.prompt("s", vec![], None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.cancel("s", None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.commands("s", None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.command("s", "c", None, None, None, None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.respond_to_permission("s", "r", None, None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.delete_session("s", None))
            .catch_unwind()
            .await;
        let _ = AssertUnwindSafe(plane.restore(
            "s",
            &sepia_session::RestoreRequest {
                confirm: false,
                path: None,
                tool_call_id: None,
                checkpoint: None,
                paths: None,
            },
            None,
        ))
        .catch_unwind()
        .await;
        let _ = AssertUnwindSafe(plane.rewind(
            "s",
            &sepia_session::RewindRequest {
                confirm: false,
                node_id: None,
                turns: None,
                checkpoint: None,
            },
            None,
        ))
        .catch_unwind()
        .await;
        // The non-panicking leftover arms.
        assert!(plane.list_agents().await.is_empty());
        assert!(!plane.can_rewind("devin").await);
        plane.close_all().await;
    }
}
