#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::pedantic,
    clippy::missing_panics_doc
)]

//! Integration tests — the engine against a real `sepia_http::app`
//! served on a loopback socket (ureq needs a wire, not `oneshot`).
//!
//! `StubNode` puts a toggling TCP proxy in front of the axum server:
//! `down()` aborts the accept loop *and* the per-connection forwarders,
//! so in-flight SSE connections die like a real crashed node — aborting
//! `axum::serve` alone leaves accepted connections (and their
//! heartbeats) alive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sepia_control::{ControlPlane, ControlPlaneOptions};
use sepia_core::storage::SessionRepository;
use sepia_core::{Session, StorageError};
use sepia_http::{AppState, EventFeed, app};
use sepia_meta::MetaStore;
use sepia_outbox::{OpKind, Outbox};
use sepia_sync::engine::SubmitOutcome;
use sepia_sync::{
    IndexedSession, NodeClient, NodeRef, NodeStatus, NodeTarget, NodeWrite, ProjectionStore,
    SyncEngine, SyncOptions,
};
use serde_json::{Value, json};

/* ---- fake store + node harness --------------------------------------*/

struct MemStore {
    sessions: Mutex<HashMap<String, Session>>,
}

#[async_trait]
impl SessionRepository for MemStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
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
    ) -> Result<Option<Session>, StorageError> {
        Ok(self.sessions.lock().unwrap().get(id).cloned())
    }
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let mut all: Vec<Session> = self.sessions.lock().unwrap().values().cloned().collect();
        all.sort_by(|a, b| b.last_activity_at.total_cmp(&a.last_activity_at));
        Ok(all)
    }
    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        self.sessions.lock().unwrap().remove(id);
        Ok(())
    }
    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.sessions.lock().unwrap().contains_key(id))
    }
}

fn session(id: &str, title: &str) -> Session {
    let mut s = sepia_testkit::contract::session(id, title, 1_700_000_100.0);
    s.backend_type = "devin".into();
    s.working_directory = "/tmp".into();
    s
}

/// Accepts on `listener` and splices each connection to `real`.
/// `kill` refuses new accepts; the task is aborted (with every tracked
/// connection task) to simulate the node vanishing.
fn spawn_proxy(
    listener: tokio::net::TcpListener,
    real: std::net::SocketAddr,
    kill: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((mut inbound, _)) = listener.accept().await else {
                return;
            };
            if kill.load(Ordering::Relaxed) {
                drop(inbound);
                continue;
            }
            let task = tokio::spawn(async move {
                let Ok(mut outbound) = tokio::net::TcpStream::connect(real).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
            conns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(task);
        }
    })
}

/// A `sepia_http` node behind a killable proxy, controllable by the test.
struct StubNode {
    /// Proxy port — the engine's target URL.
    port: u16,
    /// The real axum server's port.
    real_port: u16,
    store: Arc<MemStore>,
    feed: EventFeed,
    meta: MetaStore,
    kill: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    accept: Option<tokio::task::JoinHandle<()>>,
    _serve: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl StubNode {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn node_ref(&self, id: &str) -> NodeRef {
        NodeRef {
            id: id.into(),
            url: self.url(),
            label: format!("test {id}"),
        }
    }

    /// Simulate a crash: stop accepting and sever live connections.
    /// The port frees once the accept task unwinds.
    fn down(&mut self) {
        self.kill.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.abort();
        }
        for task in self
            .conns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }

    /// Back on the same proxy port — the previous listener was dropped
    /// by `down`'s abort (SO_REUSEADDR + a short retry cover the race).
    async fn up(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let listener = loop {
            match tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await {
                Ok(l) => break l,
                Err(e) if std::time::Instant::now() < deadline => {
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => panic!("rebind proxy port {}: {e}", self.port),
            }
        };
        self.kill.store(false, Ordering::Relaxed);
        let real = ([127, 0, 0, 1], self.real_port).into();
        self.accept = Some(spawn_proxy(
            listener,
            real,
            Arc::clone(&self.kill),
            Arc::clone(&self.conns),
        ));
    }
}

async fn start_stub(sessions: Vec<Session>) -> StubNode {
    let store = Arc::new(MemStore {
        sessions: Mutex::new(sessions.into_iter().map(|s| (s.id.clone(), s)).collect()),
    });
    let plane = ControlPlane::new(
        store.clone(),
        ControlPlaneOptions {
            idle_ttl: Some(Duration::ZERO),
            ..ControlPlaneOptions::default()
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    let state = AppState::new(plane)
        .with_meta(meta.clone())
        .with_held_watch(Duration::ZERO)
        .with_keep_alive(Duration::from_millis(40));
    let feed = state.feed.clone();

    // Real node server on an ephemeral port…
    let real_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let real_port = real_listener.local_addr().unwrap().port();
    let serve = tokio::spawn(async move {
        let _ = axum::serve(real_listener, app(state)).await;
    });

    // …behind the killable proxy the engine actually talks to.
    let proxy_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = proxy_listener.local_addr().unwrap().port();
    let kill = Arc::new(AtomicBool::new(false));
    let conns = Arc::new(Mutex::new(Vec::new()));
    let accept = spawn_proxy(
        proxy_listener,
        ([127, 0, 0, 1], real_port).into(),
        Arc::clone(&kill),
        Arc::clone(&conns),
    );

    StubNode {
        port,
        real_port,
        store,
        feed,
        meta,
        kill,
        conns,
        accept: Some(accept),
        _serve: serve,
        _dir: dir,
    }
}

fn fast_options() -> SyncOptions {
    SyncOptions {
        on_event: None,
        connect_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_secs(2),
        // Long enough that no mid-test reconnect gap drops emitted
        // events — pump teardown is still bounded by the 40ms heartbeat.
        sse_max_age: Duration::from_secs(30),
        backoff_min: Duration::from_millis(30),
        backoff_max: Duration::from_millis(200),
        resync_interval: Duration::from_secs(3600),
        feed_channel: 64,
    }
}

/// Poll `cond` until it holds or `timeout` lapses.
async fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration, what: &str) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

fn indexed(node_id: &str, session_id: &str) -> impl Fn(&ProjectionStore) -> Option<IndexedSession> {
    let node_id = node_id.to_string();
    let session_id = session_id.to_string();
    move |p: &ProjectionStore| p.session(&node_id, &session_id).unwrap()
}

fn write(
    node_id: &str,
    session_id: &str,
    op: &str,
    kind: OpKind,
    payload: Value,
    key: &str,
) -> NodeWrite {
    NodeWrite {
        node_id: node_id.into(),
        session_id: session_id.into(),
        agent: Some("devin".into()),
        op: op.into(),
        kind,
        payload,
        idempotency_key: key.into(),
        ttl_seconds: None,
    }
}

/* ---- projection store (pure) -----------------------------------------*/

#[test]
fn projection_upserts_patches_and_reconciles() {
    let store = ProjectionStore::in_memory().unwrap();
    store
        .upsert_node(&NodeRef {
            id: "n1".into(),
            url: "http://x".into(),
            label: "node one".into(),
        })
        .unwrap();
    let row = store.node("n1").unwrap().unwrap();
    assert_eq!(row.status, NodeStatus::Unknown);

    let summary = json!({
        "id": "s1", "title": "T1", "cwd": "/a", "agent": "devin",
        "source": "devin", "updatedAt": "2024-01-01T00:00:01.000Z",
        "locked": false, "busy": true, "pinned": true,
    });
    store.upsert_summary("n1", &summary).unwrap();
    let got = store.session("n1", "s1").unwrap().unwrap();
    assert_eq!(got.title, "T1");
    assert!(got.busy);
    assert_eq!(got.raw["pinned"], true);

    // Diff merge — un-patched fields survive; columns re-derive.
    store
        .apply_patch("n1", "s1", &json!({"title": "T2", "busy": false}))
        .unwrap();
    let got = store.session("n1", "s1").unwrap().unwrap();
    assert_eq!(got.title, "T2");
    assert!(!got.busy);
    assert_eq!(got.cwd, "/a");
    assert_eq!(got.raw["pinned"], true);

    // Reconcile: a listing without s1 drops it; s2 keeps it.
    store
        .upsert_summary(
            "n1",
            &json!({"id":"s2","title":"S2","updatedAt":"2024-01-02T00:00:00.000Z"}),
        )
        .unwrap();
    store
        .apply_listing(
            "n1",
            &[json!({"id":"s2","title":"S2","updatedAt":"2024-01-02T00:00:00.000Z"})],
        )
        .unwrap();
    assert!(store.session("n1", "s1").unwrap().is_none());
    assert!(store.session("n1", "s2").unwrap().is_some());
    assert_eq!(
        store.cursor("n1").unwrap().as_deref(),
        Some("2024-01-02T00:00:00.000Z")
    );

    store.set_node_status("n1", NodeStatus::Up).unwrap();
    let row = store.node("n1").unwrap().unwrap();
    assert_eq!(row.status, NodeStatus::Up);
    assert!(row.last_seen_at.is_some());
}

/* ---- full cycle -------------------------------------------------------*/

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_down_enqueue_then_up_drains_in_order() {
    let mut stub = start_stub(vec![session("s1", "S1")]).await;
    stub.down();

    let projection = ProjectionStore::in_memory().unwrap();
    let outbox = Outbox::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection.clone(),
        outbox.clone(),
        [NodeTarget::new(stub.node_ref("n1"))],
        fast_options(),
    );

    // Writes queue while the node is down.
    let queued = handle
        .submit_op(write(
            "n1",
            "s1",
            "meta.patch",
            OpKind::Metadata,
            json!({"title": "queued rename"}),
            "k1",
        ))
        .await
        .unwrap();
    assert!(matches!(queued, SubmitOutcome::Queued(_)));
    handle
        .submit_op(write(
            "n1",
            "s1",
            "meta.patch",
            OpKind::Metadata,
            json!({"pinned": true}),
            "k2",
        ))
        .await
        .unwrap();
    assert_eq!(outbox.pending_for_node("n1").unwrap().len(), 2);

    // Node comes up — the engine reconnects, lists, and drains FIFO.
    stub.up().await;
    let proj = projection.clone();
    wait_until(
        || {
            proj.node("n1")
                .unwrap()
                .is_some_and(|n| n.status == NodeStatus::Up)
        },
        Duration::from_secs(10),
        "node up",
    )
    .await;
    wait_until(
        || outbox.pending_for_node("n1").unwrap().is_empty(),
        Duration::from_secs(10),
        "outbox drained",
    )
    .await;

    // Order + effect: last-writer-wins landed both patches, in order.
    let meta = stub.meta.of("s1").unwrap();
    assert_eq!(meta.title.as_deref(), Some("queued rename"));
    assert_eq!(meta.pinned, Some(true));

    // Projection picked up the listing, and the meta-patch SSE diff
    // from the drained write propagated the overlay too.
    let got = indexed("n1", "s1")(&projection).unwrap();
    assert_eq!(got.agent, "devin");
    let proj = projection.clone();
    wait_until(
        || indexed("n1", "s1")(&proj).is_some_and(|s| s.raw["pinned"] == true),
        Duration::from_secs(5),
        "pinned propagated via SSE",
    )
    .await;

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_op_rejected_by_node_dead_letters() {
    let mut stub = start_stub(vec![session("s1", "S1")]).await;
    stub.down();

    let projection = ProjectionStore::in_memory().unwrap();
    let outbox = Outbox::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection,
        outbox.clone(),
        [NodeTarget::new(stub.node_ref("n1"))],
        fast_options(),
    );

    // A prompt queued while down — Turn kind dead-letters on replay
    // failure instead of reordering into the session. (The stub has no
    // live agent, so the node rejects it with an HTTP error.)
    handle
        .submit_op(write(
            "n1",
            "s1",
            "prompt",
            OpKind::Turn,
            json!({"text": "hi"}),
            "p1",
        ))
        .await
        .unwrap();

    stub.up().await;
    let outbox2 = outbox.clone();
    wait_until(
        || {
            outbox2
                .dead_letters()
                .unwrap()
                .iter()
                .any(|e| e.idempotency_key == "p1")
        },
        Duration::from_secs(10),
        "prompt dead-lettered (no live agent to prompt)",
    )
    .await;
    assert!(outbox.pending_for_node("n1").unwrap().is_empty());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_diffs_land_in_projection() {
    let stub = start_stub(vec![session("s1", "S1"), session("s2", "S2")]).await;
    let projection = ProjectionStore::in_memory().unwrap();
    let outbox = Outbox::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection.clone(),
        outbox,
        [NodeTarget::new(stub.node_ref("n1"))],
        fast_options(),
    );

    let proj = projection.clone();
    wait_until(
        || proj.sessions(Some("n1")).unwrap().len() == 2,
        Duration::from_secs(10),
        "initial listing in projection",
    )
    .await;

    // `session` diff — busy flag flips.
    stub.feed
        .emit_session("s1", Some("devin"), json!({"busy": true}));
    let proj = projection.clone();
    wait_until(
        || indexed("n1", "s1")(&proj).is_some_and(|s| s.busy),
        Duration::from_secs(5),
        "busy=true diff",
    )
    .await;

    // `meta` diff — exercise the real path: PATCH the node over HTTP;
    // its instrumented meta store emits the event the engine applies.
    let client = NodeClient::new(&stub.url(), None).unwrap();
    tokio::task::spawn_blocking(move || {
        client
            .post_op(
                "s1",
                Some("devin"),
                "meta.patch",
                &json!({"title": "renamed"}),
            )
            .unwrap();
    })
    .await
    .unwrap();
    let proj = projection.clone();
    wait_until(
        || indexed("n1", "s1")(&proj).is_some_and(|s| s.title == "renamed"),
        Duration::from_secs(5),
        "title diff",
    )
    .await;

    // Tombstone — `{deleted: true}` on a `session` frame removes the row.
    stub.feed
        .emit_session("s2", Some("devin"), json!({"deleted": true}));
    let proj = projection.clone();
    wait_until(
        || indexed("n1", "s2")(&proj).is_none(),
        Duration::from_secs(5),
        "delete diff",
    )
    .await;

    // A diff for a never-indexed session refetches the full summary —
    // s3 doesn't exist on the node, the refetch 404s, row stays absent.
    stub.feed
        .emit_session("s3", Some("devin"), json!({"busy": true}));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(indexed("n1", "s3")(&projection).is_none());

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_after_node_restart_resyncs() {
    let mut stub = start_stub(vec![session("s1", "S1")]).await;
    let projection = ProjectionStore::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection.clone(),
        Outbox::in_memory().unwrap(),
        [NodeTarget::new(stub.node_ref("n1"))],
        fast_options(),
    );
    let proj = projection.clone();
    wait_until(
        || {
            proj.node("n1")
                .unwrap()
                .is_some_and(|n| n.status == NodeStatus::Up)
        },
        Duration::from_secs(10),
        "initial up",
    )
    .await;

    // Kill the node — the SSE connection severs, status flips to down.
    stub.down();
    let proj = projection.clone();
    wait_until(
        || {
            proj.node("n1")
                .unwrap()
                .is_some_and(|n| n.status == NodeStatus::Down)
        },
        Duration::from_secs(10),
        "down detected",
    )
    .await;

    // "Restart" with a changed store — the reconnect's full listing
    // replaces the index (s1 tombstoned, s9 appears).
    stub.store.delete("s1").await.unwrap();
    stub.store.save(&session("s9", "S9")).await.unwrap();
    stub.up().await;
    let proj = projection.clone();
    wait_until(
        || indexed("n1", "s9")(&proj).is_some() && indexed("n1", "s1")(&proj).is_none(),
        Duration::from_secs(10),
        "resync replaced index",
    )
    .await;

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_op_posts_directly_when_node_is_up() {
    let mut stub = start_stub(vec![session("s1", "S1")]).await;
    let projection = ProjectionStore::in_memory().unwrap();
    let outbox = Outbox::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection.clone(),
        outbox.clone(),
        [NodeTarget::new(stub.node_ref("n1"))],
        fast_options(),
    );
    let proj = projection.clone();
    wait_until(
        || {
            proj.node("n1")
                .unwrap()
                .is_some_and(|n| n.status == NodeStatus::Up)
        },
        Duration::from_secs(10),
        "up",
    )
    .await;

    let outcome = handle
        .submit_op(write(
            "n1",
            "s1",
            "meta.patch",
            OpKind::Metadata,
            json!({"title": "direct"}),
            "d1",
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, SubmitOutcome::Applied(_)));
    assert!(outbox.pending_for_node("n1").unwrap().is_empty());
    assert_eq!(stub.meta.of("s1").unwrap().title.as_deref(), Some("direct"));

    // Unknown node — typed error, not a silent queue.
    let err = handle
        .submit_op(write(
            "nope",
            "s1",
            "meta.patch",
            OpKind::Metadata,
            json!({}),
            "d2",
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown node"));

    stub.down();
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_all_loops_and_pumps() {
    let mut stub = start_stub(vec![session("s1", "S1")]).await;
    // A second, permanently-down node exercises the backoff path.
    let mut dead = start_stub(vec![]).await;
    dead.down();

    let projection = ProjectionStore::in_memory().unwrap();
    let handle = SyncEngine::spawn(
        projection.clone(),
        Outbox::in_memory().unwrap(),
        [
            NodeTarget::new(stub.node_ref("up-node")),
            NodeTarget::new(dead.node_ref("dead-node")),
        ],
        fast_options(),
    );
    let proj = projection.clone();
    wait_until(
        || {
            proj.node("up-node")
                .unwrap()
                .is_some_and(|n| n.status == NodeStatus::Up)
        },
        Duration::from_secs(10),
        "up-node connected",
    )
    .await;
    wait_until(
        || handle.active_pumps() == 1,
        Duration::from_secs(5),
        "sse pump running",
    )
    .await;

    // Supervisor + both loops exit promptly; pumps unwind at the next
    // heartbeat (keep-alive is 40ms on the stub).
    let pumps = handle.pump_counter();
    tokio::time::timeout(Duration::from_secs(10), handle.shutdown())
        .await
        .expect("shutdown timed out — a loop leaked");
    wait_until(
        || pumps.load(Ordering::Relaxed) == 0,
        Duration::from_secs(2),
        "pump threads exited",
    )
    .await;

    stub.down();
    dead.down();
}
