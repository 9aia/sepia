//! [`SyncNodeApi`] — the hub's [`NodeApi`] backed by `sepia-sync`:
//! session lists read the local projection (merged across every paired
//! node), history proxies to the owning node live, and writes go
//! through `SyncEngine::submit_op` so they queue when the node is down.

use std::collections::HashMap;
use std::sync::Arc;

use sepia_sync::client::NodeClient;
use sepia_sync::{IndexedSession, NodeWrite, OpKind, SyncHandle};
use sepia_web::api::NodeApi;
use sepia_web::dto::{HistoryMessageDto, HistoryPageDto, SessionSummaryDto};
use serde_json::Value;

/// Node registry entry — url + optional bearer token.
#[derive(Clone)]
pub struct HubNode {
    pub id: String,
    pub url: String,
    pub token: Option<String>,
}

/// `SEPIA_NODES` format: `id=url[@token];id2=url2[@token2]` — the `@`
/// is unambiguous: a URL never carries one in this context.
pub fn parse_nodes(raw: &str) -> Vec<HubNode> {
    raw.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (id, rest) = part.split_once('=')?;
            let (url, token) = match rest.split_once('@') {
                Some((u, t)) => (u, if t.is_empty() { None } else { Some(t) }),
                None => (rest, None),
            };
            Some(HubNode {
                id: id.trim().to_string(),
                url: url.trim().trim_end_matches('/').to_string(),
                token: token.map(str::to_string),
            })
        })
        .collect()
}

/// The node a session belongs to — projection lookup; session ids are
/// unique per node but may collide across nodes.
fn owner<'a>(
    projection: &sepia_sync::ProjectionStore,
    id: &str,
    agent: Option<&str>,
    nodes: &'a HashMap<String, HubNode>,
) -> Result<(String, &'a HubNode), String> {
    // `agent` narrows when ids collide; otherwise the first match wins.
    let mut found: Option<String> = None;
    for row in projection
        .sessions(None)
        .map_err(|e| format!("projection: {e}"))?
    {
        if row.session_id != id {
            continue;
        }
        if let Some(want) = agent {
            if row.agent == want {
                return nodes
                    .get(&row.node_id)
                    .map(|n| (row.node_id.clone(), n))
                    .ok_or_else(|| format!("node {} not registered", row.node_id));
            }
        } else if found.is_none() {
            found = Some(row.node_id.clone());
        }
    }
    let node_id = found.ok_or_else(|| format!("Session not found: {id}"))?;
    nodes
        .get(&node_id)
        .map(|n| (node_id.clone(), n))
        .ok_or_else(|| format!("node {node_id} not registered"))
}

/// `NodeApi` over the sync projection + per-node clients.
pub struct SyncNodeApi {
    engine: SyncHandle,
    nodes: HashMap<String, HubNode>,
    clients: HashMap<String, Arc<NodeClient>>,
}

impl SyncNodeApi {
    pub fn new(engine: SyncHandle, nodes: Vec<HubNode>) -> Self {
        let clients = nodes
            .iter()
            .filter_map(|n| {
                NodeClient::new(&n.url, n.token.clone())
                    .ok()
                    .map(|c| (n.id.clone(), Arc::new(c)))
            })
            .collect();
        Self {
            engine,
            nodes: nodes.into_iter().map(|n| (n.id.clone(), n)).collect(),
            clients,
        }
    }

    fn projection(&self) -> sepia_sync::ProjectionStore {
        self.engine.projection()
    }

    fn client(&self, node_id: &str) -> Result<Arc<NodeClient>, String> {
        self.clients
            .get(node_id)
            .cloned()
            .ok_or_else(|| format!("node {node_id} not registered"))
    }

    /// The upstream `(url, token)` for a session — the proxy uses it to
    /// route session-scoped requests to the owning node.
    pub fn node_url_for(&self, id: &str, agent: Option<&str>) -> Option<(String, Option<String>)> {
        let (node_id, node) = owner(&self.projection(), id, agent, &self.nodes).ok()?;
        let _ = node_id;
        Some((node.url.clone(), node.token.clone()))
    }

    fn resolve(&self, id: &str, agent: Option<&str>) -> Result<(String, Arc<NodeClient>), String> {
        let (node_id, _) = owner(&self.projection(), id, agent, &self.nodes)?;
        Ok((node_id.clone(), self.client(&node_id)?))
    }

    async fn submit(
        &self,
        id: &str,
        agent: Option<&str>,
        op: &str,
        kind: OpKind,
        payload: Value,
    ) -> Result<(), String> {
        let (node_id, _) = owner(&self.projection(), id, agent, &self.nodes)?;
        self.engine
            .submit_op(NodeWrite {
                node_id,
                session_id: id.to_string(),
                agent: agent.map(str::to_string),
                op: op.to_string(),
                kind,
                payload,
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                ttl_seconds: None,
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[async_trait::async_trait]
impl NodeApi for SyncNodeApi {
    async fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>, String> {
        let sessions: Vec<IndexedSession> = self
            .projection()
            .sessions(None)
            .map_err(|e| format!("projection: {e}"))?;
        // The raw wire summary carries every field the DTO needs;
        // annotate the owning node so the UI can show + route it.
        Ok(sessions
            .into_iter()
            .map(|row| {
                let mut raw = row.raw.clone();
                if let Value::Object(m) = &mut raw {
                    m.insert("node".into(), Value::String(row.node_id.clone()));
                }
                serde_json::from_value(raw).unwrap_or_default()
            })
            .collect())
    }

    async fn get_session(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<SessionSummaryDto, String> {
        let (node_id, _) = owner(&self.projection(), id, agent, &self.nodes)?;
        let row = self
            .projection()
            .session(&node_id, id)
            .map_err(|e| format!("projection: {e}"))?
            .ok_or_else(|| format!("Session not found: {id}"))?;
        let mut raw = row.raw.clone();
        if let Value::Object(m) = &mut raw {
            m.insert("node".into(), Value::String(node_id));
        }
        serde_json::from_value(raw).map_err(|e| format!("decode: {e}"))
    }

    async fn history(
        &self,
        id: &str,
        agent: Option<&str>,
        before: Option<i64>,
        limit: Option<usize>,
    ) -> Result<HistoryPageDto, String> {
        let (_, client) = self.resolve(id, agent)?;
        let agent_owned = agent.map(str::to_string);
        let id_owned = id.to_string();
        let page = tokio::task::spawn_blocking(move || {
            client.get_history(&id_owned, limit, before, agent_owned.as_deref())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        Ok(HistoryPageDto {
            messages: page
                .messages
                .into_iter()
                .map(|m| serde_json::from_value(m).unwrap_or_default())
                .collect::<Vec<HistoryMessageDto>>(),
            total: page.total,
            start: page.start,
        })
    }

    async fn prompt(&self, id: &str, agent: Option<&str>, text: &str) -> Result<(), String> {
        self.submit(
            id,
            agent,
            "prompt",
            OpKind::Turn,
            serde_json::json!({ "text": text }),
        )
        .await
    }

    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), String> {
        self.submit(id, agent, "cancel", OpKind::Turn, serde_json::json!({}))
            .await
    }
}
