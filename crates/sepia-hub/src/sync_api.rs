//! [`SyncNodeApi`] — the hub's [`NodeApi`] backed by `sepia-sync`:
//! session lists read the local projection (merged across every paired
//! node), history proxies to the owning node live, and writes go
//! through `SyncEngine::submit_op` so they queue when the node is down.

use std::collections::HashMap;
use std::sync::Arc;

use std::collections::BTreeMap;

use sepia_sync::client::NodeClient;
use sepia_sync::{IndexedSession, NodeWrite, OpKind, OutboxEntry, OutboxStatus, SyncHandle};
use sepia_web::api::NodeApi;
use sepia_web::dto::{
    AgentDto, AttachResultDto, CheckpointDto, CreateResultDto, HistoryMessageDto, HistoryPageDto,
    NodeInfoDto, NodeStatusDto, PendingWriteDto, ProjectDto, PushSubscriptionDto,
    SessionSummaryDto,
};
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
    /// First configured node — the canonical target for registry
    /// writes (projects/config/push) that have no session to route by.
    primary: Option<String>,
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
            primary: nodes.first().map(|n| n.id.clone()),
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

    /// Every `(node_id, client)` pair, sorted by id for deterministic
    /// merged output.
    fn client_list(&self) -> Vec<(String, Arc<NodeClient>)> {
        let mut list: Vec<_> = self
            .clients
            .iter()
            .map(|(id, c)| (id.clone(), Arc::clone(c)))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// The client for `node` when given, else the primary's.
    fn scoped_client(&self, node: Option<&str>) -> Result<(String, Arc<NodeClient>), String> {
        let id = match node {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => self
                .primary
                .clone()
                .ok_or_else(|| "no nodes registered".to_string())?,
        };
        let client = self.client(&id)?;
        Ok((id, client))
    }

    /// `GET {path}` on every registered node, merging the `{key}`
    /// array each returns. Rows are annotated with the owning `node`
    /// id; a node that fails is skipped — unless every node fails and
    /// nothing merged, which surfaces the first error.
    async fn merged_list(&self, path: &str, key: &str) -> Result<Vec<Value>, String> {
        let clients = self.client_list();
        let path = path.to_string();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let mut first_err: Option<String> = None;
            for (node_id, client) in clients {
                match client.get_json(&path) {
                    Ok(body) => {
                        let Some(rows) = body.get(&key).and_then(Value::as_array) else {
                            continue;
                        };
                        for row in rows {
                            let mut row = row.clone();
                            if let Value::Object(m) = &mut row {
                                m.insert("node".into(), Value::String(node_id.clone()));
                            }
                            out.push(row);
                        }
                    }
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(format!("{node_id}: {e}"));
                        }
                    }
                }
            }
            match (out.is_empty(), first_err) {
                (true, Some(e)) => Err(e),
                _ => Ok(out),
            }
        })
        .await
        .map_err(|e| e.to_string())?
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

    /// `/api/sessions/{id}[/{suffix}][?agent=]` — for session-scoped ops
    /// `NodeClient::post_op` doesn't map (attach/detach/restore/…).
    fn session_path(id: &str, suffix: &str, agent: Option<&str>) -> String {
        let mut path = format!("/api/sessions/{}", sepia_sync::client::encode_segment(id));
        if !suffix.is_empty() {
            path.push('/');
            path.push_str(suffix);
        }
        if let Some(a) = agent {
            path.push_str("?agent=");
            path.push_str(&sepia_sync::client::encode_segment(a));
        }
        path
    }

    /// A session-scoped write the outbox can't carry — posted straight
    /// to the owning node (fails fast when the node is down rather than
    /// queueing a stale attach/restore).
    async fn post_direct(
        &self,
        id: &str,
        agent: Option<&str>,
        suffix: &str,
        payload: Value,
    ) -> Result<Value, String> {
        let (_, client) = self.resolve(id, agent)?;
        let path = Self::session_path(id, suffix, agent);
        tokio::task::spawn_blocking(move || client.send_json("POST", &path, &payload))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
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

/// `OutboxEntry` → the UI row — pending and in-flight entries read as
/// `queued`; dead-letters surface as `failed` with their last error.
fn pending_dto(e: &OutboxEntry) -> PendingWriteDto {
    PendingWriteDto {
        id: e.id.clone(),
        node_id: e.node_id.clone(),
        session_id: e.session_id.clone(),
        op: e.op.clone(),
        kind: match e.kind {
            OpKind::Metadata => "metadata",
            OpKind::Turn => "turn",
        }
        .to_string(),
        status: if e.status == OutboxStatus::Dead {
            "failed"
        } else {
            "queued"
        }
        .to_string(),
        enqueued_at: e.created_at.clone(),
        attempts: e.attempts,
        last_error: e.last_error.clone(),
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

    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<&str>,
        title: Option<&str>,
        model: Option<&str>,
        node: Option<&str>,
    ) -> Result<CreateResultDto, String> {
        // A create can't route by session id — `node` (or the primary)
        // picks the target. Goes direct: queueing a create in the
        // outbox would hand the UI an id nothing references yet.
        let (_, client) = self.scoped_client(node)?;
        let mut body = serde_json::json!({ "cwd": cwd });
        if let Some(a) = agent {
            body["agent"] = serde_json::json!(a);
        }
        if let Some(t) = title {
            body["title"] = serde_json::json!(t);
        }
        if let Some(m) = model {
            body["model"] = serde_json::json!(m);
        }
        tokio::task::spawn_blocking(move || client.send_json("POST", "/api/sessions", &body))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
            .and_then(|v| serde_json::from_value(v).map_err(|e| format!("decode: {e}")))
    }

    async fn attach(
        &self,
        id: &str,
        agent: Option<&str>,
        takeover: bool,
    ) -> Result<AttachResultDto, String> {
        // Direct post — attaching a session on a down node can't queue
        // meaningfully (the lock state it reacts to is now, not later).
        let body = self
            .post_direct(
                id,
                agent,
                "attach",
                serde_json::json!({ "takeover": takeover }),
            )
            .await?;
        serde_json::from_value(body).map_err(|e| format!("decode: {e}"))
    }

    async fn detach(&self, id: &str, agent: Option<&str>) -> Result<(), String> {
        self.post_direct(id, agent, "detach", serde_json::json!({}))
            .await
            .map(|_| ())
    }

    async fn answer_permission(
        &self,
        id: &str,
        agent: Option<&str>,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        // Turn-shaped like prompt/cancel: dead-letter on replay failure
        // rather than answering a request the agent already dropped.
        self.submit(
            id,
            agent,
            "permission",
            OpKind::Turn,
            serde_json::json!({ "requestId": request_id, "optionId": option_id }),
        )
        .await
    }

    async fn patch_meta(&self, id: &str, agent: Option<&str>, patch: &Value) -> Result<(), String> {
        self.submit(id, agent, "meta.patch", OpKind::Metadata, patch.clone())
            .await
    }

    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), String> {
        // Idempotent node-side (a missing session deletes cleanly), so
        // Metadata-kind retries are safe.
        self.submit(id, agent, "delete", OpKind::Metadata, serde_json::json!({}))
            .await
    }

    async fn checkpoints(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<Vec<CheckpointDto>, String> {
        let (_, client) = self.resolve(id, agent)?;
        let path = Self::session_path(id, "checkpoints", agent);
        let body = tokio::task::spawn_blocking(move || client.get_json(&path))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        Ok(body
            .get("checkpoints")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|r| serde_json::from_value(r.clone()).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn restore(&self, id: &str, agent: Option<&str>, checkpoint: &str) -> Result<(), String> {
        self.post_direct(
            id,
            agent,
            "restore",
            serde_json::json!({ "confirm": true, "checkpoint": checkpoint }),
        )
        .await
        .map(|_| ())
    }

    async fn rewind(&self, id: &str, agent: Option<&str>, checkpoint: &str) -> Result<(), String> {
        self.post_direct(
            id,
            agent,
            "rewind",
            serde_json::json!({ "confirm": true, "checkpoint": checkpoint }),
        )
        .await
        .map(|_| ())
    }

    async fn pair(&self, code: &str, node: Option<&str>) -> Result<String, String> {
        // Pairing redeems a code on one specific node — `node` picks
        // it, else the primary. Goes direct: the code is single-use
        // and queueing it in the outbox makes no sense.
        let (_, client) = self.scoped_client(node)?;
        let body = serde_json::json!({ "code": code });
        let res = tokio::task::spawn_blocking(move || client.send_json("POST", "/api/pair", &body))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| match e {
                // Surface the node's `{error}` payload, not the raw body.
                sepia_sync::client::NodeError::Http { body, .. } => {
                    serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| format!("pairing failed: {body}"))
                }
                e => e.to_string(),
            })?;
        res.get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "node returned no token".to_string())
    }

    async fn list_agents(&self) -> Result<Vec<AgentDto>, String> {
        Ok(self
            .merged_list("/api/agents", "agents")
            .await?
            .into_iter()
            .map(|row| serde_json::from_value(row).unwrap_or_default())
            .collect())
    }

    async fn list_projects(&self) -> Result<Vec<ProjectDto>, String> {
        Ok(self
            .merged_list("/api/projects", "projects")
            .await?
            .into_iter()
            .map(|row| serde_json::from_value(row).unwrap_or_default())
            .collect())
    }

    async fn create_project(&self, name: &str, node: Option<&str>) -> Result<ProjectDto, String> {
        let (node_id, client) = self.scoped_client(node)?;
        let body = serde_json::json!({ "name": name });
        let body =
            tokio::task::spawn_blocking(move || client.send_json("POST", "/api/projects", &body))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        let mut row = body.get("project").cloned().unwrap_or(Value::Null);
        if let Value::Object(m) = &mut row {
            m.insert("node".into(), Value::String(node_id));
        }
        serde_json::from_value(row).map_err(|e| format!("decode: {e}"))
    }

    async fn delete_project(&self, id: &str, node: Option<&str>) -> Result<(), String> {
        // `node` routes to the owner; absent it, idempotent-deleting on
        // every node covers the ambiguity — ids are opaque per node.
        let targets: Vec<(String, Arc<NodeClient>)> = match node {
            Some(_) => vec![self.scoped_client(node)?],
            None => self.client_list(),
        };
        let targets = if targets.is_empty() {
            return Err("no nodes registered".into());
        } else {
            targets
        };
        let path = format!("/api/projects/{}", sepia_sync::client::encode_segment(id));
        tokio::task::spawn_blocking(move || {
            let mut first_err = None;
            for (node_id, client) in targets {
                if let Err(e) = client.send_json("DELETE", &path, &Value::Null) {
                    first_err.get_or_insert(format!("{node_id}: {e}"));
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_config(&self) -> Result<BTreeMap<String, Value>, String> {
        let (_, client) = self.scoped_client(None)?;
        tokio::task::spawn_blocking(move || {
            let body = client.get_json("/api/config").map_err(|e| e.to_string())?;
            let map = body
                .get("config")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            Ok(map.into_iter().collect())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn set_config(&self, key: &str, value: &Value) -> Result<(), String> {
        let (_, client) = self.scoped_client(None)?;
        let path = format!("/api/config/{}", sepia_sync::client::encode_segment(key));
        let body = serde_json::json!({ "value": value });
        tokio::task::spawn_blocking(move || client.send_json("PATCH", &path, &body))
            .await
            .map_err(|e| e.to_string())?
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn fs_dirs(&self, path: &str, node: Option<String>) -> Result<Vec<String>, String> {
        // Read-only and node-local — goes straight to the named node
        // (or the primary) rather than the projection or outbox.
        let (_, client) = self.scoped_client(node.as_deref())?;
        let api_path = format!("/api/fs?path={}", sepia_sync::client::encode_segment(path));
        tokio::task::spawn_blocking(move || client.get_json(&api_path))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
            .map(|v| {
                v.get("dirs")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|r| r.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            })
    }

    async fn node_info(&self) -> Result<NodeInfoDto, String> {
        let (_, client) = self.scoped_client(None)?;
        tokio::task::spawn_blocking(move || client.get_json("/api/node"))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
            .and_then(|v| serde_json::from_value(v).map_err(|e| format!("decode: {e}")))
    }

    async fn rename_node(&self, name: &str, node: Option<&str>) -> Result<NodeInfoDto, String> {
        // Direct post — a rename on a down node can't queue meaningfully
        // (the identity it edits is now, not later), same as pair.
        let (_, client) = self.scoped_client(node)?;
        let body = serde_json::json!({ "name": name });
        tokio::task::spawn_blocking(move || client.send_json("PATCH", "/api/node", &body))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
            .and_then(|v| serde_json::from_value(v).map_err(|e| format!("decode: {e}")))
    }

    async fn node_status(&self) -> Result<Vec<NodeStatusDto>, String> {
        let projection = self.projection();
        let registry = self.nodes.clone();
        tokio::task::spawn_blocking(move || {
            let mut rows: Vec<NodeStatusDto> = projection
                .nodes()
                .map_err(|e| format!("projection: {e}"))?
                .into_iter()
                .map(|n| NodeStatusDto {
                    id: n.id,
                    url: n.url,
                    label: n.label,
                    status: n.status.as_str().to_string(),
                    last_seen_at: n.last_seen_at,
                })
                .collect();
            // Registered-but-never-synced nodes still get a row.
            for (id, node) in &registry {
                if !rows.iter().any(|r| &r.id == id) {
                    rows.push(NodeStatusDto {
                        id: id.clone(),
                        url: node.url.clone(),
                        label: id.clone(),
                        status: "unknown".into(),
                        last_seen_at: None,
                    });
                }
            }
            rows.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(rows)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn pending_writes(&self) -> Result<Vec<PendingWriteDto>, String> {
        // Pending entries are addressable per node; dead-letters are a
        // single list. Entries queued for a since-removed node have no
        // reader — the outbox has no scan-all for `pending`.
        let outbox = self.engine.outbox();
        let mut node_ids: Vec<String> = self.nodes.keys().cloned().collect();
        node_ids.sort_unstable();
        tokio::task::spawn_blocking(move || -> Result<Vec<PendingWriteDto>, String> {
            let mut out = Vec::new();
            for id in &node_ids {
                for entry in &outbox.pending_for_node(id).map_err(|e| e.to_string())? {
                    out.push(pending_dto(entry));
                }
            }
            for entry in &outbox.dead_letters().map_err(|e| e.to_string())? {
                out.push(pending_dto(entry));
            }
            Ok(out)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn push_vapid(&self) -> Result<String, String> {
        let (_, client) = self.scoped_client(None)?;
        tokio::task::spawn_blocking(move || client.get_json("/api/push/vapid"))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
            .and_then(|v| {
                v.get("publicKey")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| "vapid: missing publicKey".to_string())
            })
    }

    async fn push_subscribe(&self, subscription: &PushSubscriptionDto) -> Result<(), String> {
        let (_, client) = self.scoped_client(None)?;
        let body = serde_json::to_value(subscription).map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || client.send_json("POST", "/api/push/subscribe", &body))
            .await
            .map_err(|e| e.to_string())?
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn push_unsubscribe(&self, endpoint: &str) -> Result<(), String> {
        let (_, client) = self.scoped_client(None)?;
        let body = serde_json::json!({ "endpoint": endpoint });
        tokio::task::spawn_blocking(move || {
            client.send_json("DELETE", "/api/push/subscribe", &body)
        })
        .await
        .map_err(|e| e.to_string())?
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
}
