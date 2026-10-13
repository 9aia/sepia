#![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

//! Cross-node transfer (`docs/transfer.md`) — `SyncNodeApi`'s
//! export → import → overlay-copy → verify pipeline. Mock nodes are
//! tiny axum servers recording every call; the last test runs the
//! pipeline against two real `sepia-node` processes with real devin
//! stores (a lossless-IR round-trip).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, Request, State};
use axum::response::IntoResponse;
use axum::routing::{any, get};
use sepia_hub::credentials::{FileCredentials, NodeCredentials};
use sepia_hub::sync_api::{HubNode, SyncNodeApi};
use sepia_sync::{NodeTarget, SyncEngine};
use sepia_web::api::NodeApi;
use serde_json::{Value, json};

fn file_creds() -> (tempfile::TempDir, Arc<dyn NodeCredentials>) {
    let dir = tempfile::tempdir().unwrap();
    let creds = Arc::new(FileCredentials::new(dir.path().join("credentials.json")));
    (dir, creds)
}

/* ---- mock node -------------------------------------------------------*/

/// A canned node. Every request lands in `calls` as `"METHOD path"`;
/// JSON bodies collect in `bodies` for assertions on the import
/// payload shape.
#[derive(Clone, Default)]
struct MockNode {
    /// `capabilities` list served by `/api/node` — `has_import` flips
    /// the store-write flag the transfer pre-flight checks.
    has_import: bool,
    /// When `Some(id)`, the import POST 500s for that session id —
    /// `transfer_project`'s partial-failure path.
    fail_import_for: Option<String>,
    /// 500s the export route when set.
    fail_export: bool,
    calls: Arc<Mutex<Vec<String>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    /// Id the import POST mints.
    minted: String,
}

impl MockNode {
    fn new(has_import: bool) -> Self {
        Self {
            has_import,
            minted: "new-1".into(),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }

    async fn node(State(m): State<MockNode>) -> impl IntoResponse {
        let mut caps = vec!["sessions", "export"];
        if m.has_import {
            caps.push("import");
        }
        axum::Json(json!({
            "id": "mock", "name": "mock", "version": "0",
            "protocol": 1, "agents": ["devin"],
            "capabilities": caps,
        }))
    }

    async fn export(
        State(m): State<MockNode>,
        Path(id): Path<String>,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> impl IntoResponse {
        m.calls.lock().unwrap().push(format!(
            "GET /api/sessions/{id}/export?agent={:?}",
            q.get("agent")
        ));
        if m.fail_export {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": "source store exploded" })),
            )
                .into_response();
        }
        axum::Json(json!({
            "session": {
                "id": id,
                "backendType": "windsurf",
                "title": "Store title",
                "workingDirectory": "/work",
                "messages": [{"role": "user", "content": "hi"}],
            }
        }))
        .into_response()
    }

    async fn import(State(m): State<MockNode>, req: Request) -> impl IntoResponse {
        let bytes = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        m.calls
            .lock()
            .unwrap()
            .push("POST /api/sessions/import".into());
        m.bodies.lock().unwrap().push(body.clone());
        let session_id = body
            .get("session")
            .and_then(|s| s.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if m.fail_import_for.as_deref() == Some(session_id) {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({ "error": format!("import refused for {session_id}") })),
            )
                .into_response();
        }
        (
            axum::http::StatusCode::CREATED,
            axum::Json(json!({
                "id": m.minted,
                "title": body.get("title").and_then(Value::as_str).unwrap_or("T"),
                "cwd": body.get("cwd").and_then(Value::as_str).unwrap_or("/work"),
                "agent": body.get("agent").and_then(Value::as_str).unwrap_or("devin"),
            })),
        )
            .into_response()
    }

    async fn session_rest(
        State(m): State<MockNode>,
        method: axum::http::Method,
        Path(id): Path<String>,
        req: Request,
    ) -> impl IntoResponse {
        if method == axum::http::Method::PATCH {
            m.calls
                .lock()
                .unwrap()
                .push(format!("PATCH /api/sessions/{id}"));
            let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
                .await
                .unwrap();
            m.bodies
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&bytes).unwrap_or(Value::Null));
            return axum::Json(json!({})).into_response();
        }
        m.calls
            .lock()
            .unwrap()
            .push(format!("GET /api/sessions/{id}"));
        axum::Json(json!({ "id": id, "title": "copy" })).into_response()
    }

    /// Everything else (`/api/sessions` listing, `/api/events` the sync
    /// loop hits) — recorded, answered with an empty shape.
    async fn rest(State(m): State<MockNode>, req: Request) -> impl IntoResponse {
        m.calls
            .lock()
            .unwrap()
            .push(format!("{} {}", req.method(), req.uri().path()));
        axum::Json(json!({ "sessions": [] })).into_response()
    }
}

/// Spin the mock up on an ephemeral port; returns `(node, url)`.
async fn serve(m: MockNode) -> (MockNode, String) {
    let app = axum::Router::new()
        .route("/api/node", get(MockNode::node))
        .route("/api/sessions/{id}/export", get(MockNode::export))
        .route(
            "/api/sessions/import",
            axum::routing::post(MockNode::import),
        )
        .route("/api/sessions/{id}", any(MockNode::session_rest))
        .route("/api/{*rest}", any(MockNode::rest))
        .with_state(m.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (m, format!("http://{addr}"))
}

fn api_for(nodes: Vec<HubNode>) -> (SyncNodeApi, sepia_sync::ProjectionStore, tempfile::TempDir) {
    let projection = sepia_sync::ProjectionStore::in_memory().unwrap();
    let outbox = sepia_outbox::Outbox::in_memory().unwrap();
    let targets = nodes.iter().map(|n| {
        NodeTarget::new(sepia_sync::NodeRef {
            id: n.id.clone(),
            url: n.url.clone(),
            label: n.id.clone(),
        })
    });
    let engine = SyncEngine::spawn(projection.clone(), outbox, targets, Default::default());
    let (dir, creds) = file_creds();
    (SyncNodeApi::new(engine, nodes, creds), projection, dir)
}

fn seed(projection: &sepia_sync::ProjectionStore, node: &str, id: &str, cwd: &str) {
    projection
        .upsert_summary(
            node,
            &json!({
                "id": id, "title": format!("Title {id}"), "cwd": cwd,
                "agent": "devin", "source": "devin",
                "updatedAt": "2024-01-01T00:00:00Z",
            }),
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_session_runs_export_import_overlay_verify() {
    let (src, src_url) = serve(MockNode::new(true)).await;
    let (dst, dst_url) = serve(MockNode::new(true)).await;
    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: src_url,
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: dst_url,
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    // A pinned, model-pinned row — the overlay flags the copy PATCHes.
    projection
        .upsert_summary(
            "laptop",
            &json!({
                "id": "s1", "title": "Alpha", "cwd": "/work",
                "agent": "devin", "source": "devin",
                "pinned": true, "model": "swe-1.6",
                "updatedAt": "2024-01-01T00:00:00Z",
            }),
        )
        .unwrap();

    let res = api
        .transfer_session("s1", Some("devin"), "tower", Some("/other"))
        .await
        .unwrap();
    assert_eq!(res.new_session_id, "new-1");
    assert_eq!(res.source_node, "laptop");
    assert_eq!(res.target_node, "tower");
    assert_eq!(res.cwd, "/other");
    assert!(res.warnings.is_empty(), "{:?}", res.warnings);

    // Source: the export GET (sync-loop calls filter out by path).
    assert!(
        src.calls()
            .iter()
            .any(|c| c.starts_with("GET /api/sessions/s1/export")),
        "{:?}",
        src.calls()
    );

    // Target: pre-flight /api/node, import POST, overlay PATCH, verify GET.
    let dst_calls = dst.calls();
    assert!(dst_calls.iter().any(|c| *c == "POST /api/sessions/import"));
    assert!(
        dst_calls.iter().any(|c| *c == "PATCH /api/sessions/new-1"),
        "{dst_calls:?}"
    );
    assert!(
        dst_calls.iter().any(|c| *c == "GET /api/sessions/new-1"),
        "{dst_calls:?}"
    );

    // The import body carries the IR verbatim plus the overlay fields
    // and the cwd remap.
    let import_body = dst
        .bodies()
        .into_iter()
        .find(|b| b.get("session").is_some())
        .expect("import body");
    assert_eq!(import_body["agent"], "devin");
    assert_eq!(import_body["session"]["id"], "s1");
    assert_eq!(import_body["title"], "Alpha");
    assert_eq!(import_body["model"], "swe-1.6");
    assert_eq!(import_body["cwd"], "/other");

    // The overlay PATCH carried the pinned flag.
    let patch = dst
        .bodies()
        .into_iter()
        .find(|b| b.get("pinned").is_some())
        .expect("meta patch");
    assert_eq!(patch["pinned"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_session_rejects_same_node() {
    let (src, src_url) = serve(MockNode::new(true)).await;
    let nodes = vec![HubNode {
        id: "laptop".into(),
        url: src_url,
        token: None,
    }];
    let (api, projection, _dir) = api_for(nodes);
    seed(&projection, "laptop", "s1", "/work");
    let err = api
        .transfer_session("s1", None, "laptop", None)
        .await
        .unwrap_err();
    assert!(err.contains("already lives"), "{err}");
    assert!(!src.calls().iter().any(|c| c.contains("export")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_session_requires_import_capability() {
    let (_src, src_url) = serve(MockNode::new(true)).await;
    // Target advertises no store-write seam — the pre-flight fails
    // before any export bytes move.
    let (dst, dst_url) = serve(MockNode::new(false)).await;
    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: src_url,
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: dst_url,
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    seed(&projection, "laptop", "s1", "/work");
    let err = api
        .transfer_session("s1", None, "tower", None)
        .await
        .unwrap_err();
    assert!(err.contains("does not support session import"), "{err}");
    assert!(
        !dst.calls()
            .iter()
            .any(|c| *c == "POST /api/sessions/import"),
        "{:?}",
        dst.calls()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_session_surfaces_export_errors() {
    let mut failing = MockNode::new(true);
    failing.fail_export = true;
    let (_src, src_url) = serve(failing).await;
    let (_dst, dst_url) = serve(MockNode::new(true)).await;
    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: src_url,
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: dst_url,
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    seed(&projection, "laptop", "s1", "/work");
    let err = api
        .transfer_session("s1", None, "tower", None)
        .await
        .unwrap_err();
    // The node's `{error}` payload is surfaced, not the raw status.
    assert!(err.contains("source store exploded"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_project_continues_past_failures() {
    let (_src, src_url) = serve(MockNode::new(true)).await;
    let mut dst_mock = MockNode::new(true);
    dst_mock.fail_import_for = Some("b".into());
    let (_dst, dst_url) = serve(dst_mock).await;
    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: src_url,
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: dst_url,
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    seed(&projection, "laptop", "a", "/work");
    seed(&projection, "laptop", "b", "/work");
    seed(&projection, "laptop", "c", "/work");
    seed(&projection, "laptop", "elsewhere", "/elsewhere");

    let res = api
        .transfer_project("/work", "laptop", "tower", None)
        .await
        .unwrap();
    assert_eq!(res.transferred.len(), 2);
    assert_eq!(res.failed.len(), 1);
    assert_eq!(res.failed[0].session_id, "b");
    assert!(
        res.failed[0].error.contains("import refused"),
        "{:?}",
        res.failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_project_rejects_unknown_cwd_and_same_node() {
    let (_src, src_url) = serve(MockNode::new(true)).await;
    let (_dst, dst_url) = serve(MockNode::new(true)).await;
    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: src_url.clone(),
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: dst_url,
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    seed(&projection, "laptop", "a", "/work");
    let err = api
        .transfer_project("/nope", "laptop", "tower", None)
        .await
        .unwrap_err();
    assert!(err.contains("no sessions under"), "{err}");
    let err = api
        .transfer_project("/work", "laptop", "laptop", None)
        .await
        .unwrap_err();
    assert!(err.contains("already lives"), "{err}");
}

/* ---- real two-node e2e -----------------------------------------------*/

/// The full pipeline against two real `sepia-node` daemons, each with
/// its own devin store: export pulls the IR off node A, import writes
/// it into node B's store, and the copy re-reads. Session content is
/// verified field-for-field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_over_two_real_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    std::fs::create_dir_all(&driver_dir).unwrap();
    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();

    // Node A's store carries the session to move.
    let db_a = tmp.path().join("a").join("sessions.db");
    std::fs::create_dir_all(db_a.parent().unwrap()).unwrap();
    let store_a = sepia_driver_devin::store::DevinStore::open(&db_a, false).unwrap();
    let mut session =
        sepia_testkit::contract::session("xfer-1", "Capstone session", 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    sepia_core::storage::SessionRepository::save(&store_a, &session)
        .await
        .unwrap();

    let spawn_node = |name: &str, db: &std::path::Path, port: u16| {
        let home = tmp.path().join(format!("{name}-home"));
        std::fs::create_dir_all(&home).unwrap();
        std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
            .env("SEPIA_DRIVER_DIR", &driver_dir)
            .env("SEPIA_DEVIN_DB", db)
            .env("SEPIA_HOME", &home)
            .env("SEPIA_META", home.join("meta.json"))
            .env("SEPIA_NODE", home.join("node.json"))
            .env("SEPIA_PORT", port.to_string())
            .env("SEPIA_HOST", "127.0.0.1")
            .env("HOME", tmp.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let db_b = tmp.path().join("b").join("sessions.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let mut node_a = spawn_node("a", &db_a, 18820);
    let mut node_b = spawn_node("b", &db_b, 18821);

    let url_a = "http://127.0.0.1:18820".to_string();
    let url_b = "http://127.0.0.1:18821".to_string();
    let probe = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(2)))
        .build()
        .new_agent();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    for url in [&url_a, &url_b] {
        loop {
            match probe.get(format!("{url}/api/health")).call() {
                Ok(_) => break,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    let _ = node_a.kill();
                    let _ = node_b.kill();
                    panic!("node {url} never came up: {e}");
                }
            }
        }
    }

    let nodes = vec![
        HubNode {
            id: "laptop".into(),
            url: url_a.clone(),
            token: None,
        },
        HubNode {
            id: "tower".into(),
            url: url_b.clone(),
            token: None,
        },
    ];
    let (api, projection, _dir) = api_for(nodes);
    // The sync engine may already have projected the row; the explicit
    // upsert keeps the test independent of sync timing.
    projection
        .upsert_summary(
            "laptop",
            &json!({
                "id": "xfer-1", "title": "Capstone session", "cwd": session.working_directory,
                "agent": "devin", "source": "devin",
                "updatedAt": "2024-01-01T00:00:00Z",
            }),
        )
        .unwrap();

    let res = api
        .transfer_session("xfer-1", Some("devin"), "tower", Some("/remapped"))
        .await
        .unwrap();
    let _ = node_a.kill();
    let _ = node_b.kill();
    let _ = node_a.wait();
    let _ = node_b.wait();
    assert_eq!(res.source_node, "laptop");
    assert_eq!(res.target_node, "tower");
    // The target minted a fresh id — transfers are copies.
    assert!(!res.new_session_id.is_empty());
    assert_ne!(res.new_session_id, "xfer-1");
    assert!(res.warnings.is_empty(), "{:?}", res.warnings);
}
