//! Reference driver + test fixture: an in-memory session store over the
//! full `session.*` method surface. Proves the driver-wire loop end to
//! end and doubles as the smallest possible driver implementation.

use std::collections::BTreeMap;
use std::sync::Arc;

use sepia_core::storage::SessionRepository;
use sepia_core::{Session, StorageError};
use sepia_driver_sdk::{Capability, DRIVER_PROTOCOL, DriverManifest, serve_store};

struct MemStore {
    sessions: tokio::sync::RwLock<BTreeMap<String, Session>>,
}

impl MemStore {
    fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(BTreeMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl SessionRepository for MemStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        self.sessions
            .write()
            .await
            .insert(session.id.clone(), session.clone());
        Ok(())
    }

    async fn get_by_id(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        Ok(self.sessions.read().await.values().cloned().collect())
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &sepia_core::storage::NodesWindowOptions,
    ) -> Result<Option<sepia_core::storage::SessionNodeWindow>, StorageError> {
        let sessions = self.sessions.read().await;
        let Some(session) = sessions.get(id) else {
            return Ok(None);
        };
        let total = session.nodes.len();
        let before = options.before.map_or(total, |b| {
            usize::try_from(b.max(0)).unwrap_or(usize::MAX).min(total)
        });
        let start = before.saturating_sub(options.limit.unwrap_or(total).max(1));
        Ok(Some(sepia_core::storage::SessionNodeWindow {
            nodes: session.nodes[start..before].to_vec(),
            tool_call_nodes: session
                .nodes
                .iter()
                .filter(|n| !n.tool_calls.is_empty())
                .cloned()
                .collect(),
            total,
            start,
            backend_type: session.backend_type.clone(),
        }))
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.sessions.read().await.contains_key(id))
    }
}

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "memtest".into(),
        label: "In-memory test driver".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: [
            Capability::SessionStore,
            Capability::SessionWrite,
            Capability::Checkpoints,
        ]
        .into_iter()
        .collect(),
        agent_command: None,
        config_schema: serde_json::json!({}),
        backend_type: Some("memtest".into()),
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--manifest") {
        println!("{}", serde_json::to_string(&manifest()).unwrap_or_default());
        return Ok(());
    }
    serve_store(manifest(), Arc::new(MemStore::new())).await
}
