#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! In-process HTTP tests — the axum router driven through
//! `tower::ServiceExt::oneshot` against a `MemStore` + real spawned mock
//! ACP agents (same harness as `sepia-control`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use futures::StreamExt;
use sepia_acp::{AcpConnection, AgentSpec, spawn_agent};
use sepia_control::{AgentRuntime, ControlPlane, ControlPlaneOptions};
use sepia_core::storage::SessionRepository;
use sepia_core::{MessageNode, Role, Session, StorageError};
use sepia_http::{AppState, Env, Pairing, app};
use sepia_meta::MetaStore;
use sepia_testkit::contract;
use serde_json::{Value, json};
use tower::ServiceExt;

// ---------- fakes ----------------------------------------------------

struct MemStore {
    sessions: Mutex<HashMap<String, Session>>,
}

impl MemStore {
    fn new(sessions: Vec<Session>) -> Self {
        Self {
            sessions: Mutex::new(sessions.into_iter().map(|s| (s.id.clone(), s)).collect()),
        }
    }
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

struct MockRuntime {
    id: String,
    env: Vec<(String, String)>,
}

impl MockRuntime {
    fn new(id: &str, env: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        })
    }
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        "Mock"
    }
    async fn spawn(
        &self,
        cwd: &str,
        _model: Option<&str>,
        _fallbacks: Option<&[String]>,
    ) -> Result<AcpConnection, sepia_acp::rpc::RpcError> {
        let spec = AgentSpec {
            id: self.id.clone(),
            label: "Mock".into(),
            command: vec![
                sepia_testkit::ensure_mock_acp_agent()
                    .to_string_lossy()
                    .to_string(),
            ],
            env: Some(self.env.iter().cloned().collect()),
        };
        spawn_agent(
            &spec,
            &sepia_acp::SpawnOptions {
                cwd: cwd.to_string(),
                env: None,
                model: None,
                fallbacks: None,
            },
        )
        .await
    }
}

fn node(id: i64, role: Role, content: &str) -> MessageNode {
    MessageNode {
        node_id: id,
        parent_node_id: None,
        role,
        content: content.into(),
        blocks: vec![],
        tool_calls: vec![],
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1_700_000_000.0 + id as f64,
        metadata: json!(null),
    }
}

fn session_with(backend: &str, nodes: Vec<MessageNode>) -> Session {
    let mut session = contract::session("s1", "S1", 1_700_000_100.0);
    session.backend_type = backend.into();
    session.working_directory = "/tmp".into();
    session.nodes = nodes;
    session
}

fn mk_plane(
    sessions: Vec<Session>,
    agents: Vec<Arc<MockRuntime>>,
) -> (Arc<ControlPlane>, Arc<MemStore>) {
    let store = Arc::new(MemStore::new(sessions));
    let plane = ControlPlane::new(
        store.clone(),
        ControlPlaneOptions {
            agents: agents
                .into_iter()
                .map(|a| a as Arc<dyn AgentRuntime>)
                .collect(),
            default_agent_id: Some("devin".into()),
            idle_ttl: None,
            ..ControlPlaneOptions::default()
        },
    );
    (plane, store)
}

/// Router + plane; meta store lives in a fresh tempdir.
struct TestApp {
    router: Router,
    plane: Arc<ControlPlane>,
    meta: MetaStore,
    _dir: tempfile::TempDir,
}

fn mk_app(plane: &Arc<ControlPlane>) -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    let state = AppState::new(plane.clone())
        .with_meta(meta.clone())
        .with_node(sepia_http::NodeIdentity {
            id: "node_test".into(),
            name: "testbox".into(),
            version: "0.0.0".into(),
        })
        // No background probing in tests — small keep-alive for stream tests.
        .with_held_watch(Duration::ZERO)
        .with_keep_alive(Duration::from_millis(40));
    let router = app(state);
    TestApp {
        router,
        plane: plane.clone(),
        meta,
        _dir: dir,
    }
}

// ---------- request helpers ------------------------------------------

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("origin", "http://localhost:3000")
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, body: Option<Value>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("origin", "http://localhost:3000")
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(serde_json::to_vec(&b).unwrap())))
        .unwrap()
}

fn patch(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(path)
        .header("origin", "http://localhost:3000")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn del(path: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(path)
        .header("origin", "http://localhost:3000")
        .body(Body::empty())
        .unwrap()
}

fn bearer(req: Request<Body>, token: &str) -> Request<Body> {
    let mut req = req;
    req.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    req
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, headers, json)
}

// ---------- env --------------------------------------------------------

#[test]
fn env_parses_defaults_and_fail_fast() {
    let env = Env::from_map(&HashMap::new()).unwrap();
    assert_eq!(env.port, 8787);
    assert_eq!(env.host, "127.0.0.1");
    assert_eq!(env.sse_keep_alive, Duration::from_millis(15_000));
    assert_eq!(env.held_watch, Duration::from_millis(5_000));
    assert_eq!(
        env.origins,
        vec![
            "http://localhost:3000".to_string(),
            "http://127.0.0.1:3000".to_string()
        ]
    );

    // Bad port fails fast.
    let bad = HashMap::from([("SEPIA_PORT".to_string(), "abc".to_string())]);
    let err = Env::from_map(&bad).unwrap_err();
    assert!(err.to_string().contains("between 1 and 65535"));

    // Non-loopback without a token refuses to bind.
    let bad = HashMap::from([("SEPIA_HOST".to_string(), "0.0.0.0".to_string())]);
    let err = Env::from_map(&bad).unwrap_err();
    assert!(err.to_string().contains("SEPIA_TOKEN"));
    let ok = HashMap::from([
        ("SEPIA_HOST".to_string(), "0.0.0.0".to_string()),
        ("SEPIA_TOKEN".to_string(), "t".to_string()),
    ]);
    assert!(Env::from_map(&ok).is_ok());

    // Zero keep-alive disables it.
    let zero = HashMap::from([("SEPIA_SSE_KEEPALIVE_MS".to_string(), "0".to_string())]);
    assert_eq!(Env::from_map(&zero).unwrap().sse_keep_alive, Duration::ZERO);
}

// ---------- health / cors / 404 ---------------------------------------

#[tokio::test]
async fn health_is_ok_without_auth() {
    let (plane, _) = mk_plane(vec![], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);
    let (status, _, body) = call(&app.router, get("/api/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true, "db": true }));
    app.plane.close_all().await;
}

#[tokio::test]
async fn cors_preflight_answers_204_and_allow_headers() {
    let (plane, _) = mk_plane(vec![], vec![]);
    let app = mk_app(&plane);
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/sessions")
        .header("origin", "http://localhost:3000")
        .body(Body::empty())
        .unwrap();
    let res = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        res.headers().get("access-control-allow-origin").unwrap(),
        "http://localhost:3000"
    );
    assert_eq!(
        res.headers().get("access-control-allow-methods").unwrap(),
        "GET,POST,PATCH,DELETE,OPTIONS"
    );
    assert_eq!(
        res.headers().get("access-control-allow-headers").unwrap(),
        "content-type,authorization"
    );
    app.plane.close_all().await;
}

#[tokio::test]
async fn unknown_route_is_404() {
    let (plane, _) = mk_plane(vec![], vec![]);
    let app = mk_app(&plane);
    let (status, _, body) = call(&app.router, get("/api/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({ "error": "Not found" }));
    app.plane.close_all().await;
}

// ---------- auth -------------------------------------------------------

#[tokio::test]
async fn bearer_gate_rejects_and_accepts() {
    let (plane, _) = mk_plane(vec![], vec![]);
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(plane.clone())
        .with_token(Some("secret".into()))
        .with_meta(MetaStore::open(&dir.path().join("meta.json")))
        .with_held_watch(Duration::ZERO);
    let app = app(state);

    // Missing token → 401 with WWW-Authenticate.
    let res = app.clone().oneshot(get("/api/sessions")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(res.headers().get("www-authenticate").unwrap(), "Bearer");
    let (_, _, body) = call(&app, get("/api/sessions")).await;
    assert_eq!(body["error"], "Unauthorized");

    // Wrong token → 401.
    let (status, _, _) = call(&app, bearer(get("/api/sessions"), "wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Right token → 200.
    let (status, _, _) = call(&app, bearer(get("/api/sessions"), "secret")).await;
    assert_eq!(status, StatusCode::OK);

    // Health is exempt.
    let (status, _, _) = call(&app, get("/api/health")).await;
    assert_eq!(status, StatusCode::OK);

    // Query-string token on a normal route — rejected (401).
    let (status, _, _) = call(&app, get("/api/sessions?access_token=secret")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Cookie auth works.
    let req = {
        let mut r = get("/api/sessions");
        r.headers_mut()
            .insert("cookie", HeaderValue::from_static("sepia_token=secret"));
        r
    };
    let (status, _, _) = call(&app, req).await;
    assert_eq!(status, StatusCode::OK);

    // Login with the right token sets the cookie; logout expires it.
    let (status, headers, _) = call(
        &app,
        post("/api/auth/login", Some(json!({ "token": "secret" }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(cookie.starts_with("sepia_token=secret"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));

    let (status, headers, _) = call(&app, post("/api/auth/logout", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    // Bad login → 401.
    let (status, _, _) = call(
        &app,
        post("/api/auth/login", Some(json!({ "token": "nope" }))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    plane.close_all().await;
}

#[tokio::test]
async fn pair_redeems_code_into_bearer_token() {
    let (plane, _) = mk_plane(vec![], vec![]);
    let dir = tempfile::tempdir().unwrap();
    let code_path = dir.path().join("pair-code");
    let tokens_path = dir.path().join("tokens.json");
    let pairing = Pairing::open(code_path.clone(), tokens_path);
    // The mint channel is the pair-code file — same file `sepia pair` writes.
    let code = sepia_http::pair::mint_pair_code();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    std::fs::write(
        &code_path,
        json!({ "code": code, "expiresAt": expires_at }).to_string(),
    )
    .unwrap();

    let state = AppState::new(plane.clone())
        .with_token(Some("secret".into()))
        .with_meta(MetaStore::open(&dir.path().join("meta.json")))
        .with_pairing(pairing)
        .with_held_watch(Duration::ZERO);
    let app = app(state);

    // Bad code → the generic 404.
    let (status, _, body) = call(
        &app,
        post("/api/pair", Some(json!({ "code": "XXXX-XXXX" }))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Invalid or expired pairing code");

    // Missing code → 400.
    let (status, _, _) = call(&app, post("/api/pair", Some(json!({})))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Good code → a paired bearer token that authenticates.
    let (status, _, body) = call(&app, post("/api/pair", Some(json!({ "code": code })))).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    assert!(token.starts_with("sepia_"));
    let (status, _, _) = call(&app, bearer(get("/api/sessions"), token)).await;
    assert_eq!(status, StatusCode::OK);

    // One-time use → second redemption is the same generic 404.
    let (status, _, _) = call(&app, post("/api/pair", Some(json!({ "code": code })))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Tokens file holds only the hash.
    let tokens = std::fs::read_to_string(dir.path().join("tokens.json")).unwrap();
    assert!(!tokens.contains(token));

    plane.close_all().await;
}

// ---------- sessions ---------------------------------------------------

#[tokio::test]
async fn session_list_shape_and_meta_overlay() {
    let s1 = session_with(
        "windsurf",
        vec![node(1, Role::User, "hi"), node(2, Role::Assistant, "ok")],
    );
    let (plane, _) = mk_plane(vec![s1], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);

    // Record meta: rename + pin + project.
    app.meta.patch(
        "s1",
        &sepia_meta::SessionMeta {
            title: Some("Renamed".into()),
            pinned: Some(true),
            ..sepia_meta::SessionMeta::default()
        },
    );
    // A pending (meta-only) session — agent+cwd set but no store row.
    app.meta.patch(
        "ghost",
        &sepia_meta::SessionMeta {
            agent: Some("devin".into()),
            cwd: Some("/tmp".into()),
            created_at: Some("2024-01-01T00:00:00.000Z".into()),
            ..sepia_meta::SessionMeta::default()
        },
    );

    let (status, _, body) = call(&app.router, get("/api/sessions")).await;
    assert_eq!(status, StatusCode::OK);
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let s1 = sessions.iter().find(|s| s["id"] == "s1").unwrap();
    assert_eq!(s1["title"], "Renamed");
    assert_eq!(s1["pinned"], true);
    assert_eq!(s1["archived"], false);
    assert_eq!(s1["projectIds"], json!([]));
    assert_eq!(s1["spans"], json!([]));
    assert_eq!(s1["agent"], "devin");
    assert_eq!(s1["source"], "devin");
    assert_eq!(s1["busy"], false);
    assert_eq!(s1["locked"], false);
    assert_eq!(s1["lockHolderPid"], Value::Null);
    let ghost = sessions.iter().find(|s| s["id"] == "ghost").unwrap();
    assert_eq!(ghost["title"], "New session");
    assert_eq!(ghost["agent"], "devin");
    assert_eq!(ghost["source"], "sepia");

    // withLocks=1 merges the agent's locked flags.
    let (status, _, body) = call(&app.router, get("/api/sessions?withLocks=1")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["sessions"].is_array());

    app.plane.close_all().await;
}

#[tokio::test]
async fn history_paging_and_validation() {
    let s1 = session_with(
        "devin",
        vec![
            node(1, Role::User, "a"),
            node(2, Role::Assistant, "b"),
            node(3, Role::User, "c"),
            node(4, Role::Assistant, "d"),
        ],
    );
    let (plane, _) = mk_plane(vec![s1], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);

    let (status, _, body) = call(&app.router, get("/api/sessions/s1/history")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 4);
    assert_eq!(body["start"], 0);
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);

    let (_, _, body) = call(&app.router, get("/api/sessions/s1/history?limit=2")).await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    let (_, _, body) = call(
        &app.router,
        get("/api/sessions/s1/history?before=2&limit=1"),
    )
    .await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);

    // Bad params → 400.
    let (status, _, _) = call(&app.router, get("/api/sessions/s1/history?limit=-1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app.router, get("/api/sessions/s1/history?limit=1.5")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app.router, get("/api/sessions/s1/history?before=x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown session → the ControlError 404 shape.
    let (status, _, body) = call(&app.router, get("/api/sessions/ghost/history")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    app.plane.close_all().await;
}

#[tokio::test]
async fn session_get_export_and_meta_patch() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(vec![s1], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);

    let (status, _, body) = call(&app.router, get("/api/sessions/s1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "S1");
    assert_eq!(body["id"], "s1");

    // Export → session IR.
    let (status, _, body) = call(&app.router, get("/api/sessions/s1/export")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session"]["id"], "s1");

    // Meta PATCH + alias.
    let (status, _, _) = call(
        &app.router,
        patch(
            "/api/sessions/s1/meta",
            json!({ "title": "T2", "pinned": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app.router,
        patch(
            "/api/sessions/s1",
            json!({ "archived": true, "projectIds": ["p1"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, body) = call(&app.router, get("/api/sessions/s1")).await;
    assert_eq!(body["title"], "T2");
    assert_eq!(body["pinned"], true);
    assert_eq!(body["archived"], true);
    assert_eq!(body["projectIds"], json!(["p1"]));

    // Validation failures.
    for body in [
        json!({}),
        json!({ "title": "" }),
        json!({ "pinned": "yes" }),
        json!({ "projectIds": [1] }),
        json!({ "model": 42 }),
    ] {
        let (status, _, _) = call(&app.router, patch("/api/sessions/s1", body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // model: null clears the override.
    let (status, _, _) = call(
        &app.router,
        patch("/api/sessions/s1", json!({ "model": "m", "x": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app.router,
        patch("/api/sessions/s1", json!({ "model": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, body) = call(&app.router, get("/api/sessions/s1")).await;
    assert_eq!(body["model"], Value::Null);

    // DELETE tolerates missing sessions.
    let (status, _, body) = call(&app.router, del("/api/sessions/ghost")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true }));
    // And really deletes via the agent's session/delete RPC (the mock
    // acks it; the repo row is the agent's own store).
    let (status, _, _) = call(&app.router, del("/api/sessions/s1")).await;
    assert_eq!(status, StatusCode::OK);

    app.plane.close_all().await;
}

#[tokio::test]
async fn prompt_restore_rewind_validation() {
    let s1 = session_with("devin", vec![]);
    let (plane, _) = mk_plane(vec![s1], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);

    // prompt: empty body, non-record, empty parts, bad attachments.
    for (path, body) in [
        ("/api/sessions/s1/prompt", json!({})),
        ("/api/sessions/s1/prompt", json!({ "text": 5 })),
        ("/api/sessions/s1/prompt", json!({ "attachments": "x" })),
        (
            "/api/sessions/s1/prompt",
            json!({ "attachments": [{"type": "nope"}] }),
        ),
        ("/api/sessions/s1/permission", json!({})),
        ("/api/sessions/s1/restore", json!({ "confirm": true })),
        ("/api/sessions/s1/restore", json!({ "path": "/x" })),
        (
            "/api/sessions/s1/restore",
            json!({ "confirm": true, "path": "/x", "paths": [] }),
        ),
        ("/api/sessions/s1/rewind", json!({ "confirm": true })),
        (
            "/api/sessions/s1/rewind",
            json!({ "confirm": true, "nodeId": 1, "turns": 2 }),
        ),
        (
            "/api/sessions/s1/rewind",
            json!({ "confirm": true, "nodeId": -1 }),
        ),
        (
            "/api/sessions/s1/rewind",
            json!({ "confirm": true, "turns": 0 }),
        ),
    ] {
        let (status, _, _) = call(&app.router, post(path, Some(body))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {status}");
    }

    // convert is a no-op seam by default → 501.
    let (status, _, _) = call(
        &app.router,
        post(
            "/api/sessions/s1/convert",
            Some(json!({ "agent": "cline" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);

    // Not-attached prompt → ControlError invalid → 400 with code.
    let (status, _, body) = call(
        &app.router,
        post("/api/sessions/s1/prompt", Some(json!({ "text": "hi" }))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid");

    // cancel on an unattached session → invalid → 400.
    let (status, _, body) = call(&app.router, post("/api/sessions/ghost/cancel", None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid");

    // detach unknown → ok (plane tolerates).
    let (status, _, _) = call(&app.router, post("/api/sessions/ghost/detach", None)).await;
    assert_eq!(status, StatusCode::OK);

    app.plane.close_all().await;
}

#[tokio::test]
async fn attach_prompt_and_stream_happy_path() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![s1],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_CAPS", "load"), ("MOCK_SESSIONS", "s1|/tmp|S1|now")],
        )],
    );
    let app = mk_app(&plane);

    // attach → attached: true with capabilities.
    let (status, _, body) = call(&app.router, post("/api/sessions/s1/attach", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["attached"], true);
    assert_eq!(body["readOnly"], false);
    assert_eq!(body["agentId"], "devin");
    assert_eq!(body["capabilities"]["loadSession"], true);

    // Span recorded in meta.
    let spans = app.meta.of("s1").and_then(|m| m.spans).unwrap_or_default();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].agent, "devin");

    // Open the SSE stream, then prompt, then read frames.
    let stream_req = get("/api/sessions/s1/stream?agent=devin");
    let res = app.router.clone().oneshot(stream_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let mut frames = res.into_body().into_data_stream();

    let (status, _, body) = call(
        &app.router,
        post("/api/sessions/s1/prompt", Some(json!({ "text": "hello" }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true }));

    // Read until we see runStarted and the echo chunk.
    let mut saw_run = false;
    let mut saw_echo = false;
    for _ in 0..40 {
        let frame = tokio::time::timeout(Duration::from_secs(5), frames.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = String::from_utf8_lossy(&frame);
        if text.contains("\"runStarted\"") {
            saw_run = true;
        }
        if text.contains("echo: hello") {
            saw_echo = true;
            break;
        }
    }
    assert!(saw_run, "expected a runStarted frame");
    assert!(saw_echo, "expected the mock echo chunk");

    // detach → ok.
    let (status, _, _) = call(&app.router, post("/api/sessions/s1/detach", None)).await;
    assert_eq!(status, StatusCode::OK);

    app.plane.close_all().await;
}

#[tokio::test]
async fn attach_locked_read_only_and_takeover_error() {
    let s1 = session_with("devin", vec![]);
    let (plane, _) = mk_plane(
        vec![s1],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/tmp|S1|now|locked|4242")],
        )],
    );
    let app = mk_app(&plane);

    // Held session → read-only attach.
    let (status, _, body) = call(&app.router, post("/api/sessions/s1/attach", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["attached"], false);
    assert_eq!(body["readOnly"], true);

    // Takeover under a load failure stays locked → ControlError locked → 409.
    plane.close_all().await;
    let (plane2, _) = mk_plane(
        vec![session_with("devin", vec![])],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_SESSIONS", "s1|/tmp|S1|now|locked|4242"),
                ("MOCK_LOAD_FAIL", "held"),
            ],
        )],
    );
    let app2 = mk_app(&plane2);
    let (status, _, body) = call(
        &app2.router,
        post("/api/sessions/s1/attach", Some(json!({ "takeover": true }))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "locked");

    app2.plane.close_all().await;
}

#[tokio::test]
async fn error_codes_map_across_routes() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![s1],
        vec![
            MockRuntime::new(
                "devin",
                &[("MOCK_CAPS", "load"), ("MOCK_SESSIONS", "s1|/tmp|S1|now")],
            ),
            MockRuntime::new("cline", &[("MOCK_CAPS", "load")]),
        ],
    );
    let app = mk_app(&plane);

    // unknown_agent → 400.
    let (status, _, body) = call(
        &app.router,
        post(
            "/api/sessions",
            Some(json!({ "cwd": "/tmp", "agent": "bogus" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unknown_agent");

    // conflict → 409: attach s1 under devin, then attach ?agent=cline.
    let (status, _, _) = call(&app.router, post("/api/sessions/s1/attach", None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = call(
        &app.router,
        post("/api/sessions/s1/attach?agent=cline", None),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");

    // not_found → 404 with code on several routes.
    for path in [
        "/api/sessions/ghost",
        "/api/sessions/ghost/history",
        "/api/sessions/ghost/checkpoints",
        "/api/sessions/ghost/export",
    ] {
        let (status, _, body) = call(&app.router, get(path)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body["code"], "not_found", "{path}");
    }

    // busy → 409: two overlapping prompts (mock sleeps in prompt).
    let (plane2, _) = mk_plane(
        vec![session_with("devin", vec![])],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_CAPS", "load"),
                ("MOCK_SESSIONS", "s1|/tmp|S1|now"),
                ("MOCK_PROMPT_SLOW_MS", "400"),
            ],
        )],
    );
    let app2 = mk_app(&plane2);
    call(&app2.router, post("/api/sessions/s1/attach", None)).await;
    let router = app2.router.clone();
    let first = tokio::spawn(async move {
        router
            .oneshot(post(
                "/api/sessions/s1/prompt",
                Some(json!({ "text": "slow" })),
            ))
            .await
            .unwrap()
            .status()
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let (status, _, body) = call(
        &app2.router,
        post("/api/sessions/s1/prompt", Some(json!({ "text": "raced" }))),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "busy");
    assert_eq!(first.await.unwrap(), StatusCode::OK);

    app.plane.close_all().await;
    app2.plane.close_all().await;
}

#[tokio::test]
async fn create_session_records_meta_and_shape() {
    let (plane, _) = mk_plane(vec![], vec![MockRuntime::new("devin", &[])]);
    let app = mk_app(&plane);

    // Validation.
    for body in [
        json!(null),
        json!({}),
        json!({ "cwd": "  " }),
        json!({ "cwd": "/tmp", "agent": 5 }),
        json!({ "cwd": "/tmp", "fallbacks": [1] }),
    ] {
        let req = if body.is_null() {
            post("/api/sessions", None)
        } else {
            post("/api/sessions", Some(body))
        };
        let (status, _, _) = call(&app.router, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, _, body) = call(
        &app.router,
        post(
            "/api/sessions",
            Some(json!({ "cwd": "/tmp", "agent": "devin", "title": "Mine", "model": "m1" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["agentId"], "devin");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(id, "mock-session");

    // Meta recorded agent/cwd/model/title/createdAt.
    let meta = app.meta.of(&id).unwrap();
    assert_eq!(meta.agent.as_deref(), Some("devin"));
    assert_eq!(meta.cwd.as_deref(), Some("/tmp"));
    assert_eq!(meta.model, Some(Some("m1".to_string())));
    assert_eq!(meta.title.as_deref(), Some("Mine"));

    // The pending row shows up in the list even though the store is empty.
    let (_, _, body) = call(&app.router, get("/api/sessions")).await;
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap()
        .clone();
    assert_eq!(row["title"], "Mine");
    assert_eq!(row["agent"], "devin");

    app.plane.close_all().await;
}

// ---------- agents / projects / config / misc ---------------------------

#[tokio::test]
async fn agents_projects_config_and_misc() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![s1],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/tmp|S1|now")],
        )],
    );
    let app = mk_app(&plane);

    // Capabilities only surface after a live attach has advertised them.
    let (status, _, body) = call(&app.router, get("/api/agents")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["agents"][0]["capabilities"].is_null());
    let (status, _, _) = call(&app.router, post("/api/sessions/s1/attach", None)).await;
    assert_eq!(status, StatusCode::OK);

    // agents — { agents: [{id,label,capabilities?}] }.
    let (status, _, body) = call(&app.router, get("/api/agents")).await;
    assert_eq!(status, StatusCode::OK);
    let agents = body["agents"].as_array().unwrap();
    assert_eq!(agents[0]["id"], "devin");
    assert_eq!(agents[0]["label"], "Mock");
    assert_eq!(agents[0]["capabilities"]["loadSession"], true);

    // node descriptor.
    let (status, _, body) = call(&app.router, get("/api/node")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "node_test");
    assert_eq!(body["name"], "testbox");
    assert_eq!(body["protocol"], 1);
    assert_eq!(body["agents"], json!(["devin"]));
    let caps = body["capabilities"].as_array().unwrap();
    assert!(caps.len() >= 5);
    assert!(!caps.contains(&json!("transfer")));

    // user info shape.
    let (status, _, body) = call(&app.router, get("/api/user")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["user"]["username"].is_string());
    assert!(body["user"]["homedir"].is_string());
    assert_eq!(body["user"]["platform"], json!("linux"));

    // fs — absolute required; dirs only.
    let (status, _, _) = call(&app.router, get("/api/fs?path=relative")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, body) = call(&app.router, get("/api/fs?path=/tmp")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["dirs"].is_array());
    let (status, _, _) = call(&app.router, get("/api/fs?path=/nonexistent-xyz")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // projects CRUD.
    let (status, _, body) = call(
        &app.router,
        post("/api/projects", Some(json!({ "name": "Web" }))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let pid = body["project"]["id"].as_str().unwrap().to_string();
    let (status, _, body) = call(&app.router, get("/api/projects")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["projects"].as_array().unwrap()[0]["name"], "Web");
    let (status, _, _) = call(
        &app.router,
        patch(&format!("/api/projects/{pid}"), json!({ "name": "Site" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app.router,
        patch("/api/projects/nope", json!({ "name": "X" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app.router, del(&format!("/api/projects/{pid}"))).await;
    assert_eq!(status, StatusCode::OK);

    // config — internal keys hidden and rejected.
    let (status, _, _) = call(
        &app.router,
        patch("/api/config/theme", json!({ "value": "dark" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app.router,
        patch("/api/config/vapid", json!({ "value": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, body) = call(&app.router, get("/api/config")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["config"]["theme"], "dark");
    assert!(body["config"]["vapid"].is_null());

    // client keypair.
    let (status, _, body) = call(&app.router, post("/api/client/keypair", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["algorithm"], "Ed25519");
    assert!(body["publicKey"].as_str().unwrap().len() == 43);
    assert!(body["secretKey"].as_str().unwrap().len() == 43);

    // wrong method → 404 (TS fallthrough, not 405).
    let (status, _, _) = call(&app.router, post("/api/agents", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    app.plane.close_all().await;
}

#[tokio::test]
async fn meta_dependent_routes_501_without_store() {
    let (plane, _) = mk_plane(vec![session_with("devin", vec![])], vec![]);
    // No meta → renames/projects/config answer 501.
    let state = AppState::new(plane.clone()).with_held_watch(Duration::ZERO);
    let app = app(state);
    let (status, _, _) = call(&app, patch("/api/sessions/s1", json!({ "title": "x" }))).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let (status, _, _) = call(&app, get("/api/projects")).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let (status, _, _) = call(&app, get("/api/config")).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    plane.close_all().await;
}

#[tokio::test]
async fn node_patch_renames_and_persists() {
    let (plane, _) = mk_plane(vec![], vec![MockRuntime::new("devin", &[])]);
    let dir = tempfile::tempdir().unwrap();
    let node_path = dir.path().join("node.json");
    let state = AppState::new(plane.clone())
        .with_node(sepia_http::NodeIdentity {
            id: "node_test".into(),
            name: "testbox".into(),
            version: "0.0.0".into(),
        })
        .with_node_path(node_path.clone())
        .with_held_watch(Duration::ZERO);
    let app = app(state);

    // Validation — missing/empty/overlong names and non-object bodies.
    for body in [
        json!({}),
        json!({ "name": "" }),
        json!({ "name": "   " }),
        json!({ "name": 42 }),
        json!({ "name": "x".repeat(101) }),
        json!("just-a-string"),
    ] {
        let (status, _, _) = call(&app, patch("/api/node", body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    // Rename → the updated descriptor, and GET reflects it.
    let (status, _, body) = call(&app, patch("/api/node", json!({ "name": "  renamed  " }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "node_test");
    assert_eq!(body["name"], "renamed");
    assert_eq!(body["protocol"], 1);
    let (status, _, body) = call(&app, get("/api/node")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "renamed");

    // The identity file persisted the rename.
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&node_path).unwrap()).unwrap();
    assert_eq!(on_disk["id"], "node_test");
    assert_eq!(on_disk["name"], "renamed");

    plane.close_all().await;
}

#[tokio::test]
async fn import_convert_seams() {
    use futures::FutureExt;
    use sepia_http::{ConvertSession, ImportSession};

    // Without convert configured → both routes 501.
    let (plane, _) = mk_plane(vec![session_with("devin", vec![])], vec![]);
    let tapp = mk_app(&plane);
    let (status, _, body) = call(
        &tapp.router,
        post("/api/sessions/import", Some(json!({ "agent": "cline" }))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body["error"].as_str().unwrap().contains("Import"));

    // With convert + import wired → history form lands as a new session.
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    let import: ImportSession =
        Arc::new(|session, _target| async move { Ok(session.id.clone()) }.boxed());
    let convert: ConvertSession =
        Arc::new(|_id, _target| async move { Ok("converted-id".to_string()) }.boxed());
    let state = AppState::new(plane.clone())
        .with_meta(meta)
        .with_import_session(import)
        .with_convert(convert)
        .with_held_watch(Duration::ZERO);
    let app = app(state);

    let (status, _, body) = call(
        &app,
        post(
            "/api/sessions/import",
            Some(json!({
                "agent": "cline",
                "history": [{ "role": "user", "content": "hello world", "createdAt": 1700000000000.0 }]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["agent"], "cline");
    assert_eq!(body["source"], "cline");
    assert_eq!(body["title"], "hello world");
    assert_eq!(body["locked"], false);
    assert_eq!(body["spans"].as_array().unwrap().len(), 1);

    // Malformed history → 400.
    let (status, _, _) = call(
        &app,
        post(
            "/api/sessions/import",
            Some(json!({
                "agent": "cline",
                "history": [{ "role": "bogus", "content": "x", "createdAt": 1 }]
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // convert wired → {sessionId}.
    let (status, _, body) = call(
        &app,
        post(
            "/api/sessions/s1/convert",
            Some(json!({ "agent": "cline" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessionId"], "converted-id");

    plane.close_all().await;
}

#[tokio::test]
async fn feed_emits_meta_and_session_events() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(vec![s1], vec![MockRuntime::new("devin", &[])]);
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    let state = AppState::new(plane.clone())
        .with_meta(meta)
        .with_held_watch(Duration::ZERO);
    let feed = state.feed.clone();
    let mut rx = feed.subscribe();
    let app = app(state);

    // PATCH meta → "meta" event; DELETE → "session" {deleted:true}.
    call(&app, patch("/api/sessions/s1", json!({ "pinned": true }))).await;
    call(&app, del("/api/sessions/s1")).await;

    let e1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.kind, "meta");
    assert_eq!(e1.payload["id"], "s1");
    assert_eq!(e1.payload["patch"]["pinned"], true);
    let e2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e2.kind, "session");
    assert_eq!(e2.payload["patch"]["deleted"], true);

    plane.close_all().await;
}

#[tokio::test]
async fn query_token_works_on_sse_paths() {
    let s1 = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![s1],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_CAPS", "load"), ("MOCK_SESSIONS", "s1|/tmp|S1|now")],
        )],
    );
    let state = AppState::new(plane.clone())
        .with_token(Some("secret".into()))
        .with_held_watch(Duration::ZERO)
        .with_keep_alive(Duration::ZERO);
    let app = app(state);

    // Attach first so the stream opens with a live conn.
    let res = app
        .clone()
        .oneshot(bearer(post("/api/sessions/s1/attach", None), "secret"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // ?access_token works on the session stream.
    let res = app
        .clone()
        .oneshot(get("/api/sessions/s1/stream?access_token=secret"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    drop(res);

    // ...and on the global feed — reads a heartbeat-shaped stream.
    let res = app
        .clone()
        .oneshot(get("/api/events?access_token=secret"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let frame = res.into_body().into_data_stream();
    drop(frame);

    // Wrong query token → 401 on those paths too.
    let res = app
        .clone()
        .oneshot(get("/api/events?access_token=nope"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    plane.close_all().await;
}
