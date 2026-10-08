//! The `SessionRepository` port — the trait every store driver implements
//! and every consumer programs against.

use async_trait::async_trait;

use crate::domain::{MessageNode, Session, StorageError};

/// A paged node window an adapter can serve without parsing a session's
/// whole backlog. `nodes` is the window `start..start+len`;
/// `tool_call_nodes` carries the nodes whose `tool_calls` the window's
/// tool rows reference (the calls themselves may live outside the
/// window); `total` is the full node count.
#[derive(Clone, Debug)]
pub struct SessionNodeWindow {
    pub nodes: Vec<MessageNode>,
    pub tool_call_nodes: Vec<MessageNode>,
    pub total: usize,
    pub start: usize,
    /// Store backend — merged repos use it for `agent_id` narrowing.
    pub backend_type: String,
}

/// Options for a paged history read.
#[derive(Clone, Debug, Default)]
pub struct NodesWindowOptions {
    pub limit: Option<usize>,
    pub before: Option<i64>,
    /// Single-store implementations ignore this; merged repositories use
    /// it to resolve an id that may collide across agents.
    pub agent_id: Option<String>,
}

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), StorageError>;

    /// `agent_id` only matters for merged repositories — see
    /// [`NodesWindowOptions::agent_id`].
    async fn get_by_id(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError>;

    async fn list(&self) -> Result<Vec<Session>, StorageError>;

    /// Paged history read — adapters that can page natively override it.
    /// `Ok(None)` means "session unknown to this store" (same contract as
    /// `get_by_id`); the default falls back to `get_by_id`.
    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        let session = self.get_by_id(id, options.agent_id.as_deref()).await?;
        Ok(session.map(|s| {
            let total = s.nodes.len();
            SessionNodeWindow {
                nodes: s.nodes,
                tool_call_nodes: Vec::new(),
                total,
                start: 0,
                backend_type: s.backend_type.clone(),
            }
        }))
    }

    /// Metadata-only read — a `Session` with empty `nodes`/`prompt_history`,
    /// enough for existence checks, attach headers and checkpoint refs.
    /// Default falls back to `get_by_id`.
    async fn summary(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        self.get_by_id(id, agent_id).await.map(|s| {
            s.map(|mut s| {
                s.nodes = Vec::new();
                s.prompt_history = Vec::new();
                s
            })
        })
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError>;

    async fn has_session(&self, id: &str) -> Result<bool, StorageError>;
}

/// The tables sepia reads and writes; an existing Devin store already has them.
pub const REQUIRED_TABLES: [&str; 3] = ["sessions", "message_nodes", "prompt_history"];

/// A store that already carries the schema must not be migrated again.
pub fn needs_migration(table_names: &std::collections::BTreeSet<String>) -> bool {
    !REQUIRED_TABLES
        .iter()
        .all(|table| table_names.contains(*table))
}
