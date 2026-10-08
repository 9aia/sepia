#![allow(dead_code)]
// Shared harness extracted from control.rs — the MemStore repo, the
// mock-agent-spawning runtime, and the session builders.
#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Control-plane tests against real spawned mock ACP agents — the
//! `AgentRuntime` fake only controls the spec's env, so lock probes,
//! capability gates, and the permission round-trip all go through real
//! subprocesses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sepia_acp::{AcpConnection, AgentSpec, spawn_agent};
use sepia_control::{AgentRuntime, ControlPlane, ControlPlaneOptions};
use sepia_core::storage::SessionRepository;
use sepia_core::{MessageNode, Role, Session, StorageError};
use sepia_testkit::contract;
use serde_json::json;

// ---------- fakes ----------------------------------------------------

/// In-memory `SessionRepository` — the control plane's store port.
pub struct MemStore {
    sessions: Mutex<HashMap<String, Session>>,
}

impl MemStore {
    pub fn new(sessions: Vec<Session>) -> Self {
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

/// A runtime that spawns the real mock ACP agent binary with a custom
/// env (caps, sessions list, failure modes).
pub struct MockRuntime {
    id: String,
    env: Vec<(String, String)>,
    pub spawns: AtomicUsize,
}

impl MockRuntime {
    pub fn new(id: &str, env: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            spawns: AtomicUsize::new(0),
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
        self.spawns.fetch_add(1, Ordering::SeqCst);
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

pub fn node(id: i64, role: Role, content: &str) -> MessageNode {
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

pub fn session_with(backend: &str, nodes: Vec<MessageNode>) -> Session {
    let mut session = contract::session("s1", "S1", 1_700_000_100.0);
    session.backend_type = backend.into();
    session.working_directory = "/tmp".into(); // a real cwd — spawn chdirs into it
    session.nodes = nodes;
    session
}

pub fn mk_plane(
    sessions: Vec<Session>,
    agents: Vec<Arc<MockRuntime>>,
    mut options: ControlPlaneOptions,
) -> (Arc<ControlPlane>, Arc<MemStore>) {
    let store = Arc::new(MemStore::new(sessions));
    options.agents = agents
        .into_iter()
        .map(|a| a as Arc<dyn AgentRuntime>)
        .collect();
    (ControlPlane::new(store.clone(), options), store)
}

pub fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
