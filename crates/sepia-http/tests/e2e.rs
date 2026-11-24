//! End-to-end coverage for the session REST surface — real spawned
//! mock ACP agents behind `LocalPlane`/`ControlPlane`, requests driven
//! through `sepia_http::app` via `tower::oneshot`. Covers attach,
//! prompt, history, checkpoints and rewind plus a route-table parity
//! fixture pinning the `/api/sessions*` paths, verbs and payload
//! shapes the surface must keep serving.
#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use sepia_acp::{AcpConnection, AgentSpec, spawn_agent};
use sepia_core::rewind::RewindPlan;
use sepia_core::storage::SessionRepository;
use sepia_core::{MessageNode, Role, Session, StorageError};
use sepia_http::SessionState;
use sepia_session::{
    AgentRuntime, ControlError, ControlPlane, ControlPlaneOptions, LocalPlane, SessionRewinder,
};
use sepia_testkit::contract;
use serde_json::{Value, json};
use tower::ServiceExt;

// ---------- fakes (mirrors sepia-session's tests/common) --------------

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

/// A runtime spawning the real mock ACP agent with a custom env.
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
        model: Option<&str>,
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
                model: model.map(str::to_string),
                fallbacks: None,
            },
        )
        .await
    }
}

/// Records rewind truncations — proves the route reached the plane's
/// rewinder seam.
struct RecordingRewinder {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl SessionRewinder for RecordingRewinder {
    async fn truncate(
        &self,
        session: &Session,
        _plan: &RewindPlan,
        _truncated: &Session,
    ) -> Result<(), ControlError> {
        self.calls.lock().unwrap().push(session.id.clone());
        Ok(())
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

fn plane_with(
    sessions: Vec<Session>,
    agents: Vec<Arc<MockRuntime>>,
    options: ControlPlaneOptions,
) -> (Arc<ControlPlane>, Arc<RecordingRewinder>) {
    let store = Arc::new(MemStore::new(sessions));
    let rewinder = Arc::new(RecordingRewinder {
        calls: Mutex::new(Vec::new()),
    });
    let mut options = options;
    options.agents = agents
        .into_iter()
        .map(|a| a as Arc<dyn AgentRuntime>)
        .collect();
    options.rewinders.insert(
        "devin".to_string(),
        rewinder.clone() as Arc<dyn SessionRewinder>,
    );
    (ControlPlane::new(store, options), rewinder)
}

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(b) => Body::from(b.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn session_state(plane: Arc<ControlPlane>) -> SessionState {
    SessionState::new(Arc::new(LocalPlane(plane, None)))
}

// ---------- e2e --------------------------------------------------------

#[tokio::test]
async fn attach_prompt_history_checkpoints_rewind() {
    let session = session_with(
        "devin",
        vec![node(1, Role::User, "hi"), node(2, Role::Assistant, "hello")],
    );
    let runtime = MockRuntime::new("devin", &[("MOCK_SESSIONS", "s1|/work|S1|now")]);
    let (plane, rewinder) =
        plane_with(vec![session], vec![runtime], ControlPlaneOptions::default());
    let app = sepia_http::app(session_state(plane));

    // GET collection lists the seeded session.
    let (status, body) = call(&app, Method::GET, "/api/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sessions"].as_array().unwrap().len(), 1);

    // Attach — spawns the mock agent and loads the session.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/sessions/s1/attach",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["attached"], true);
    assert_eq!(body["agentId"], "devin");

    // Prompt — the mock agent answers the turn.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/sessions/s1/prompt",
        Some(json!({ "text": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // History — paginated transcript.
    let (status, body) = call(&app, Method::GET, "/api/sessions/s1/history", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["messages"].as_array().unwrap().len() >= 2);

    // Checkpoints — the session's snapshot refs (empty is fine).
    let (status, body) = call(&app, Method::GET, "/api/sessions/s1/checkpoints", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["checkpoints"].is_array());

    // Rewind — `confirm: true` + one target reaches the registered
    // rewinder.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/sessions/s1/rewind",
        Some(json!({ "confirm": true, "nodeId": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(*rewinder.calls.lock().unwrap(), vec!["s1".to_string()]);
}

#[tokio::test]
async fn rewind_requires_confirm() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let runtime = MockRuntime::new("devin", &[("MOCK_SESSIONS", "s1|/work|S1|now")]);
    let (plane, _) = plane_with(vec![session], vec![runtime], ControlPlaneOptions::default());
    let app = sepia_http::app(session_state(plane));
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/sessions/s1/rewind",
        Some(json!({ "nodeId": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ---------- route-table parity ------------------------------------------

/// Every path+verb the session surface must keep serving. A request
/// that reaches a handler never answers the router fallback's bare
/// `{"error":"Not found"}` — wrong methods and unknown sessions
/// answer the `{error, code?}` taxonomy instead, so the assertion
/// distinguishes "route mounted" from "route gone".
#[tokio::test]
async fn route_table_parity() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let runtime = MockRuntime::new("devin", &[("MOCK_SESSIONS", "s1|/work|S1|now")]);
    let (plane, _) = plane_with(vec![session], vec![runtime], ControlPlaneOptions::default());
    let app = sepia_http::app(session_state(plane));

    // (method, path, body) — bodies are the minimal shape each verb
    // parses far enough to leave the router.
    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/agents", None),
        (Method::GET, "/api/sessions", None),
        (
            Method::POST,
            "/api/sessions",
            Some(json!({ "cwd": "/tmp", "agent": "devin" })),
        ),
        (Method::POST, "/api/sessions/import", Some(json!({}))),
        (
            Method::POST,
            "/api/sessions/run",
            Some(json!({ "cwd": "/tmp", "agent": "devin", "text": "hi" })),
        ),
        (Method::GET, "/api/sessions/s1", None),
        (
            Method::PATCH,
            "/api/sessions/s1/meta",
            Some(json!({ "title": "x" })),
        ),
        (Method::DELETE, "/api/sessions/s1", None),
        (Method::GET, "/api/sessions/s1/history", None),
        (Method::GET, "/api/sessions/s1/checkpoints", None),
        (Method::GET, "/api/sessions/s1/export", None),
        (Method::GET, "/api/sessions/s1/stream", None),
        (Method::POST, "/api/sessions/s1/attach", Some(json!({}))),
        (Method::POST, "/api/sessions/s1/detach", Some(json!({}))),
        (
            Method::POST,
            "/api/sessions/s1/prompt",
            Some(json!({ "text": "hi" })),
        ),
        (Method::POST, "/api/sessions/s1/cancel", Some(json!({}))),
        (
            Method::POST,
            "/api/sessions/s1/permission",
            Some(json!({ "requestId": "r" })),
        ),
        (
            Method::POST,
            "/api/sessions/s1/permissions",
            Some(json!({ "requestId": "r" })),
        ),
        (
            Method::POST,
            "/api/sessions/s1/restore",
            Some(json!({ "confirm": true, "path": "/tmp/x" })),
        ),
        (
            Method::POST,
            "/api/sessions/s1/rewind",
            Some(json!({ "confirm": true, "turns": 1 })),
        ),
        (
            Method::POST,
            "/api/sessions/s1/convert",
            Some(json!({ "agent": "cline" })),
        ),
        (Method::GET, "/api/config", None),
        (
            Method::PATCH,
            "/api/config/some.key",
            Some(json!({ "value": "v" })),
        ),
        (Method::GET, "/api/projects", None),
        (Method::POST, "/api/projects", Some(json!({ "name": "p" }))),
        (
            Method::PATCH,
            "/api/projects/p1",
            Some(json!({ "name": "p" })),
        ),
    ];

    for (method, path, body) in cases {
        let (status, value) = call(&app, method.clone(), path, body.clone()).await;
        let error = value.get("error").and_then(Value::as_str).unwrap_or("");
        assert_ne!(
            error, "Not found",
            "{method} {path} hit the router fallback — route missing (status {status}, body {value})"
        );
        // Every mounted route answers inside the HTTP taxonomy.
        assert!(
            status.is_success() || status.is_client_error() || status.is_server_error(),
            "{method} {path} returned a non-HTTP status {status}"
        );
    }

    // And the fallback itself still answers bare 404s.
    let (status, value) = call(&app, Method::GET, "/api/sessions/nope/stream", None).await;
    // `nope` isn't a session — the stream handler's subscribe error is
    // the taxonomy, not the fallback; a truly unknown path is:
    let (status2, value2) = call(&app, Method::GET, "/api/definitely-not-a-route", None).await;
    assert_eq!(status2, StatusCode::NOT_FOUND);
    assert_eq!(value2["error"], "Not found");
    let _ = (status, value);
}
