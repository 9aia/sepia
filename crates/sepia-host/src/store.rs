//! `RemoteStore` — a `SessionRepository` over one driver subprocess —
//! and `MergedStore`, the cross-driver union consumers see.

use std::sync::Arc;

use async_trait::async_trait;
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use sepia_core::{Session, StorageError};
use sepia_driver::{methods, rpc};
use serde_json::json;

use crate::client::DriverClient;

/// A `SessionRepository` that forwards every call to a driver subprocess.
#[derive(Clone)]
pub struct RemoteStore {
    client: Arc<DriverClient>,
    /// The agent/backend id sessions from this store carry (`agent_id`
    /// narrowing for the merged view).
    pub agent_id: String,
}

impl RemoteStore {
    pub fn new(client: Arc<DriverClient>, agent_id: String) -> Self {
        Self { client, agent_id }
    }

    pub fn client(&self) -> &Arc<DriverClient> {
        &self.client
    }

    /// `config.read` — the agent's config IR under `dir` (`None` → the
    /// driver resolves its own default).
    ///
    /// # Errors
    /// `CAPABILITY_UNSUPPORTED` when the driver lacks `AgentConfig`.
    pub async fn read_config(
        &self,
        dir: Option<&std::path::Path>,
    ) -> Result<sepia_core::agent_config::AgentConfig, sepia_driver::rpc::RpcError> {
        let value = self
            .client
            .call(methods::CONFIG_READ, json!({ "dir": dir }))
            .await?;
        serde_json::from_value(value).map_err(|e| rpc::RpcError::internal(e.to_string()))
    }

    /// `config.write` — install a config IR payload under `dir` (`None`
    /// → the driver's default); returns the per-file write report.
    ///
    /// # Errors
    /// `CAPABILITY_UNSUPPORTED` when the driver lacks `AgentConfig`;
    /// `INVALID_PARAMS` on a malformed `config` payload.
    pub async fn write_config(
        &self,
        dir: Option<&std::path::Path>,
        config: &serde_json::Value,
    ) -> Result<Vec<sepia_core::agent_config::ConfigWriteAction>, sepia_driver::rpc::RpcError> {
        let value = self
            .client
            .call(
                methods::CONFIG_WRITE,
                json!({ "dir": dir, "config": config }),
            )
            .await?;
        let result: methods::ConfigWriteResult =
            serde_json::from_value(value).map_err(|e| rpc::RpcError::internal(e.to_string()))?;
        Ok(result.actions)
    }

    /// `session.rewind` — the control plane ships the pre-rewind IR, the
    /// computed cut, and the truncated IR; the driver picks its store
    /// mechanism (row delete, transcript slice, checkpoint re-root).
    ///
    /// # Errors
    /// `CAPABILITY_UNSUPPORTED` when the driver has no rewinder.
    pub async fn rewind(
        &self,
        session: &sepia_core::Session,
        plan: &sepia_core::rewind::RewindPlan,
        truncated: &sepia_core::Session,
    ) -> Result<(), sepia_driver::rpc::RpcError> {
        self.client
            .call(
                sepia_driver::methods::SESSION_REWIND,
                serde_json::json!({
                    "session": session,
                    "plan": plan,
                    "truncated": truncated,
                }),
            )
            .await?;
        Ok(())
    }

    fn err(e: rpc::RpcError) -> StorageError {
        StorageError::new(e.message)
    }
}

#[async_trait]
impl SessionRepository for RemoteStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        self.client
            .call(
                methods::SESSION_SAVE,
                serde_json::to_value(session).unwrap_or_default(),
            )
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    async fn get_by_id(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        let result = self
            .client
            .call(
                methods::SESSION_GET,
                json!({ "id": id, "agentId": agent_id }),
            )
            .await;
        match result {
            Ok(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| StorageError::new(e.to_string())),
            Err(e) if e.code == rpc::SESSION_NOT_FOUND => Ok(None),
            Err(e) => Err(Self::err(e)),
        }
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let value = self
            .client
            .call(methods::SESSION_LIST, json!({}))
            .await
            .map_err(Self::err)?;
        serde_json::from_value(value).map_err(|e| StorageError::new(e.to_string()))
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        let result = self
            .client
            .call(
                methods::SESSION_HISTORY,
                json!({
                    "id": id,
                    "limit": options.limit,
                    "before": options.before,
                    "agentId": options.agent_id,
                }),
            )
            .await;
        match result {
            Ok(value) => serde_json::from_value::<RemoteWindow>(value)
                .map(|w| {
                    Some(SessionNodeWindow {
                        nodes: w.nodes,
                        tool_call_nodes: w.tool_call_nodes,
                        total: w.total,
                        start: w.start,
                        backend_type: w.backend_type,
                    })
                })
                .map_err(|e| StorageError::new(e.to_string())),
            Err(e) if e.code == rpc::SESSION_NOT_FOUND => Ok(None),
            Err(e) => Err(Self::err(e)),
        }
    }

    async fn summary(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        let result = self
            .client
            .call(
                methods::SESSION_SUMMARY,
                json!({ "id": id, "agentId": agent_id }),
            )
            .await;
        match result {
            Ok(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| StorageError::new(e.to_string())),
            Err(e) if e.code == rpc::SESSION_NOT_FOUND => Ok(None),
            Err(e) => Err(Self::err(e)),
        }
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        self.client
            .call(methods::SESSION_DELETE, json!({ "id": id }))
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.summary(id, None).await?.is_some())
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteWindow {
    nodes: Vec<sepia_core::MessageNode>,
    #[serde(default)]
    tool_call_nodes: Vec<sepia_core::MessageNode>,
    total: usize,
    start: usize,
    backend_type: String,
}

/// The union view: every SessionStore driver's sessions merged, with
/// `agent_id` disambiguating ids that collide across stores.
pub struct MergedStore {
    stores: Vec<RemoteStore>,
}

impl MergedStore {
    pub fn new(stores: Vec<RemoteStore>) -> Self {
        Self { stores }
    }

    /// The store that owns `agent_id` — for `?agent=` scoped ops.
    pub fn for_agent(&self, agent_id: &str) -> Option<&RemoteStore> {
        self.stores.iter().find(|s| s.agent_id == agent_id)
    }

    fn pick(&self, agent_id: Option<&str>) -> Vec<&RemoteStore> {
        match agent_id {
            Some(id) => self.for_agent(id).into_iter().collect(),
            None => self.stores.iter().collect(),
        }
    }
}

#[async_trait]
impl SessionRepository for MergedStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        let target = session
            .agent_id
            .as_deref()
            .and_then(|id| self.for_agent(id))
            .or_else(|| self.stores.first());
        match target {
            Some(store) => store.save(session).await,
            None => Err(StorageError::new("no store drivers installed")),
        }
    }

    async fn get_by_id(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        for store in self.pick(agent_id) {
            match store.get_by_id(id, agent_id).await {
                Ok(Some(session)) => return Ok(Some(session)),
                Ok(None) => {}
                Err(e) => {
                    if agent_id.is_some() {
                        return Err(e);
                    }
                }
            }
        }
        Ok(None)
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        // Partial data beats total failure: a crashed driver yields its
        // zero sessions, not an error for the aggregate.
        let mut merged = Vec::new();
        for store in &self.stores {
            match store.list().await {
                Ok(sessions) => merged.extend(sessions),
                Err(e) => {
                    eprintln!("sepia: store {} list failed: {e}", store.agent_id);
                }
            }
        }
        Ok(merged)
    }

    async fn summary(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        for store in self.pick(agent_id) {
            match store.summary(id, agent_id).await {
                Ok(Some(session)) => return Ok(Some(session)),
                Ok(None) => {}
                Err(e) => {
                    if agent_id.is_some() {
                        return Err(e);
                    }
                }
            }
        }
        Ok(None)
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        for store in self.pick(options.agent_id.as_deref()) {
            match store.nodes_window(id, options).await {
                Ok(Some(window)) => return Ok(Some(window)),
                Ok(None) => {}
                Err(e) => {
                    if options.agent_id.is_some() {
                        return Err(e);
                    }
                }
            }
        }
        Ok(None)
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        for store in &self.stores {
            if store.has_session(id).await.unwrap_or(false) {
                return store.delete(id).await;
            }
        }
        Err(StorageError::new(format!("unknown session {id}")))
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        for store in &self.stores {
            if store.has_session(id).await.unwrap_or(false) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
