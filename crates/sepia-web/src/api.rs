//! Server functions — the browser never talks to the node directly.
//! Each `#[server]` fn runs on the hub (`ssr` builds: inline during SSR,
//! as an HTTP endpoint under `/hub/*` when called from wasm) and calls
//! the node API through the [`NodeApi`] port. `sepia-hub` provides the
//! client as context; unit tests substitute a stub.

// The `#[server]` macro's client-side trait impl generates `async` fns
// with no `.await` — not ours to fix.
#![allow(clippy::unused_async_trait_impl)]

use std::collections::BTreeMap;

use leptos::prelude::*;
use serde_json::Value;

use crate::dto::{
    AgentDto, HistoryPageDto, NodeInfoDto, NodeStatusDto, ProjectDto, PushSubscriptionDto,
    SessionSummaryDto,
};

/// Port to the node's `/api/*` surface — implemented by [`HttpNodeApi`]
/// in production and by stubs in tests.
#[cfg(feature = "ssr")]
#[async_trait::async_trait]
pub trait NodeApi: Send + Sync + 'static {
    /// `GET /api/sessions`.
    async fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>, String>;
    /// `GET /api/sessions/{id}/history?before=&limit=&agent=`.
    async fn history(
        &self,
        id: &str,
        agent: Option<&str>,
        before: Option<i64>,
        limit: Option<usize>,
    ) -> Result<HistoryPageDto, String>;
    /// `GET /api/sessions/{id}`.
    async fn get_session(&self, id: &str, agent: Option<&str>)
    -> Result<SessionSummaryDto, String>;
    /// `POST /api/sessions/{id}/prompt` — `{text}`.
    async fn prompt(&self, id: &str, agent: Option<&str>, text: &str) -> Result<(), String>;
    /// `POST /api/sessions/{id}/cancel`.
    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), String>;
    /// `GET /api/agents`.
    async fn list_agents(&self) -> Result<Vec<AgentDto>, String>;
    /// `GET /api/projects`.
    async fn list_projects(&self) -> Result<Vec<ProjectDto>, String>;
    /// `POST /api/projects` — `{name}`. `node` scopes the create to one
    /// registered node; `None` = the primary/first.
    async fn create_project(&self, name: &str, node: Option<&str>) -> Result<ProjectDto, String>;
    /// `DELETE /api/projects/{id}` — `node` routes to the owning node
    /// when ids collide across a merged registry.
    async fn delete_project(&self, id: &str, node: Option<&str>) -> Result<(), String>;
    /// `GET /api/config` — the public config map (internal keys like
    /// `vapid`/`pushSubscriptions` are filtered node-side).
    async fn get_config(&self) -> Result<BTreeMap<String, Value>, String>;
    /// `PATCH /api/config/{key}` — `{value}` stored verbatim.
    async fn set_config(&self, key: &str, value: &Value) -> Result<(), String>;
    /// `GET /api/node` — the node descriptor (the primary node's on a
    /// multi-node hub).
    async fn node_info(&self) -> Result<NodeInfoDto, String>;
    /// Per-node health rows — the sync projection's `nodes` table on a
    /// hub, a one-row `GET /api/node` probe on a direct connection.
    async fn node_status(&self) -> Result<Vec<NodeStatusDto>, String>;
    /// `GET /api/push/vapid` — `{publicKey}`.
    async fn push_vapid(&self) -> Result<String, String>;
    /// `POST /api/push/subscribe` — `{endpoint, keys}`.
    async fn push_subscribe(&self, subscription: &PushSubscriptionDto) -> Result<(), String>;
    /// `DELETE /api/push/subscribe` — `{endpoint}`.
    async fn push_unsubscribe(&self, endpoint: &str) -> Result<(), String>;
}

#[cfg(feature = "ssr")]
fn node_api() -> Result<std::sync::Arc<dyn NodeApi>, ServerFnError> {
    use_context::<std::sync::Arc<dyn NodeApi>>()
        .ok_or_else(|| ServerFnError::new("hub: node API client missing from context"))
}

/// `GET /api/sessions` — every session the node can see.
#[server(prefix = "/hub")]
pub async fn list_sessions() -> Result<Vec<SessionSummaryDto>, ServerFnError> {
    node_api()?
        .list_sessions()
        .await
        .map_err(ServerFnError::new)
}

/// `GET /api/sessions/{id}/history` — one page. `before` is the node
/// index the next page should end before (the previous page's `start`).
#[server(prefix = "/hub")]
pub async fn session_history(
    session_id: String,
    agent: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
) -> Result<HistoryPageDto, ServerFnError> {
    if session_id.is_empty() {
        return Err(ServerFnError::new("session id is required"));
    }
    node_api()?
        .history(
            &session_id,
            agent.as_deref(),
            before,
            limit.and_then(|l| usize::try_from(l).ok()),
        )
        .await
        .map_err(ServerFnError::new)
}

/// `GET /api/sessions/{id}` — the summary row for one session.
#[server(prefix = "/hub")]
pub async fn get_session(
    session_id: String,
    agent: Option<String>,
) -> Result<SessionSummaryDto, ServerFnError> {
    if session_id.is_empty() {
        return Err(ServerFnError::new("session id is required"));
    }
    node_api()?
        .get_session(&session_id, agent.as_deref())
        .await
        .map_err(ServerFnError::new)
}

/// `POST /api/sessions/{id}/prompt`.
#[server(prefix = "/hub")]
pub async fn send_prompt(
    session_id: String,
    agent: Option<String>,
    text: String,
) -> Result<(), ServerFnError> {
    if session_id.is_empty() || text.trim().is_empty() {
        return Err(ServerFnError::new(
            "a session id and non-empty text are required",
        ));
    }
    node_api()?
        .prompt(&session_id, agent.as_deref(), &text)
        .await
        .map_err(ServerFnError::new)
}

/// `POST /api/sessions/{id}/cancel`.
#[server(prefix = "/hub")]
pub async fn cancel_run(session_id: String, agent: Option<String>) -> Result<(), ServerFnError> {
    node_api()?
        .cancel(&session_id, agent.as_deref())
        .await
        .map_err(ServerFnError::new)
}

/// `GET /api/agents`.
#[server(prefix = "/hub")]
pub async fn list_agents() -> Result<Vec<AgentDto>, ServerFnError> {
    node_api()?.list_agents().await.map_err(ServerFnError::new)
}

/// `GET /api/projects`.
#[server(prefix = "/hub")]
pub async fn list_projects() -> Result<Vec<ProjectDto>, ServerFnError> {
    node_api()?
        .list_projects()
        .await
        .map_err(ServerFnError::new)
}

/// `POST /api/projects` — `{name}`; `node` pins the create to one
/// registered node on a multi-node hub.
#[server(prefix = "/hub")]
pub async fn create_project(
    name: String,
    node: Option<String>,
) -> Result<ProjectDto, ServerFnError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ServerFnError::new(
            "name must be a non-empty string (max 100)",
        ));
    }
    node_api()?
        .create_project(name, node.as_deref())
        .await
        .map_err(ServerFnError::new)
}

/// `DELETE /api/projects/{id}`.
#[server(prefix = "/hub")]
pub async fn delete_project(project_id: String, node: Option<String>) -> Result<(), ServerFnError> {
    if project_id.is_empty() {
        return Err(ServerFnError::new("project id is required"));
    }
    node_api()?
        .delete_project(&project_id, node.as_deref())
        .await
        .map_err(ServerFnError::new)
}

/// `GET /api/config` — the node's public config map.
#[server(prefix = "/hub")]
pub async fn get_config() -> Result<BTreeMap<String, Value>, ServerFnError> {
    node_api()?.get_config().await.map_err(ServerFnError::new)
}

/// `PATCH /api/config/{key}` — `{value}`.
#[server(prefix = "/hub")]
pub async fn set_config(key: String, value: Value) -> Result<(), ServerFnError> {
    if key.trim().is_empty() {
        return Err(ServerFnError::new("config key is required"));
    }
    node_api()?
        .set_config(&key, &value)
        .await
        .map_err(ServerFnError::new)
}

/// `GET /api/node` — the primary node's descriptor.
#[server(prefix = "/hub")]
pub async fn node_info() -> Result<NodeInfoDto, ServerFnError> {
    node_api()?.node_info().await.map_err(ServerFnError::new)
}

/// Per-node health rows — the hub registry + sync engine status.
#[server(prefix = "/hub")]
pub async fn node_status() -> Result<Vec<NodeStatusDto>, ServerFnError> {
    node_api()?.node_status().await.map_err(ServerFnError::new)
}

/// `GET /api/push/vapid` — the public key push subscriptions are made
/// against.
#[server(prefix = "/hub")]
pub async fn push_vapid_key() -> Result<String, ServerFnError> {
    node_api()?.push_vapid().await.map_err(ServerFnError::new)
}

/// `POST /api/push/subscribe` — register a browser push endpoint.
#[server(prefix = "/hub")]
pub async fn push_subscribe(subscription: PushSubscriptionDto) -> Result<(), ServerFnError> {
    if subscription.endpoint.is_empty() {
        return Err(ServerFnError::new("push endpoint is required"));
    }
    node_api()?
        .push_subscribe(&subscription)
        .await
        .map_err(ServerFnError::new)
}

/// `DELETE /api/push/subscribe` — drop a browser push endpoint.
#[server(prefix = "/hub")]
pub async fn push_unsubscribe(endpoint: String) -> Result<(), ServerFnError> {
    if endpoint.is_empty() {
        return Err(ServerFnError::new("push endpoint is required"));
    }
    node_api()?
        .push_unsubscribe(&endpoint)
        .await
        .map_err(ServerFnError::new)
}

/* ---- ssr: HTTP client ------------------------------------------------*/

/// `NodeApi` backed by the node daemon's HTTP API (`ureq` — the same
/// client `sepia-node`'s e2e tests use). Calls are blocking I/O, so
/// each trait method hops onto `spawn_blocking`.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct HttpNodeApi {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

#[cfg(feature = "ssr")]
impl HttpNodeApi {
    /// `base` is the node's origin, e.g. `http://127.0.0.1:8787`.
    pub fn new(base: impl Into<String>, token: Option<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .build();
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token,
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let url = format!("{}{}", self.base, path);
        let mut req = match method {
            "DELETE" => self.agent.delete(&url),
            "HEAD" => self.agent.head(&url),
            _ => self.agent.get(&url),
        };
        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req
    }

    fn send_json(
        &self,
        method: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let url = format!("{}{}", self.base, path);
        let mut req = match method {
            "PATCH" => self.agent.patch(&url),
            "PUT" => self.agent.put(&url),
            _ => self.agent.post(&url),
        };
        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req.send_json(body).map_err(|e| e.to_string())
    }

    /// `DELETE` with a JSON body — ureq's `delete` builder is
    /// body-less, so the request is assembled by hand.
    fn delete_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let url = format!("{}{}", self.base, path);
        let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let mut builder = ureq::http::Request::builder().method("DELETE").uri(&url);
        if let Some(token) = &self.token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        let req = builder
            .header("Content-Type", "application/json")
            .body(bytes)
            .map_err(|e| e.to_string())?;
        self.agent.run(req).map_err(|e| e.to_string())
    }

    fn get(&self, path: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
        self.request("GET", path).call().map_err(|e| e.to_string())
    }

    /// The single-node registry row for an unreachable upstream.
    fn down_row(&self) -> NodeStatusDto {
        NodeStatusDto {
            id: "node".into(),
            url: self.base.clone(),
            label: self.base.clone(),
            status: "down".into(),
            last_seen_at: None,
        }
    }
}

/// `Err` carries the node's `{error}` payload when there is one.
#[cfg(feature = "ssr")]
fn error_of(mut res: ureq::http::Response<ureq::Body>, what: &str) -> String {
    let status = res.status().as_u16();
    let message = res
        .body_mut()
        .read_json::<serde_json::Value>()
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_string)));
    match message {
        Some(m) => m,
        None => format!("{what} failed (HTTP {status})"),
    }
}

/// Read a JSON body, or surface the node's `{error}` payload.
#[cfg(feature = "ssr")]
fn read_json<T: serde::de::DeserializeOwned>(
    res: ureq::http::Response<ureq::Body>,
    what: &str,
) -> Result<T, String> {
    if res.status().is_success() {
        let mut res = res;
        return res.body_mut().read_json::<T>().map_err(|e| e.to_string());
    }
    Err(error_of(res, what))
}

#[cfg(feature = "ssr")]
fn id_query(agent: Option<&str>) -> String {
    agent
        .map(|a| format!("?agent={}", url_encode(a)))
        .unwrap_or_default()
}

/// Minimal percent-encoding for the `?agent` query param (store ids are
/// path-safe, but agent ids are free-form config keys).
#[cfg(feature = "ssr")]
fn url_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(feature = "ssr")]
fn history_path(
    id: &str,
    agent: Option<&str>,
    before: Option<i64>,
    limit: Option<usize>,
) -> String {
    let mut path = format!("/api/sessions/{id}/history");
    let mut params = Vec::new();
    if let Some(a) = agent {
        params.push(format!("agent={}", url_encode(a)));
    }
    if let Some(b) = before {
        params.push(format!("before={b}"));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if !params.is_empty() {
        path.push('?');
        path.push_str(&params.join("&"));
    }
    path
}

#[cfg(feature = "ssr")]
#[async_trait::async_trait]
impl NodeApi for HttpNodeApi {
    async fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct SessionsBody {
                #[serde(default)]
                sessions: Vec<SessionSummaryDto>,
            }
            read_json::<SessionsBody>(api.get("/api/sessions")?, "list sessions")
                .map(|b| b.sessions)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn history(
        &self,
        id: &str,
        agent: Option<&str>,
        before: Option<i64>,
        limit: Option<usize>,
    ) -> Result<HistoryPageDto, String> {
        let path = history_path(id, agent, before, limit);
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            read_json::<HistoryPageDto>(api.get(&path)?, "get history")
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_session(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<SessionSummaryDto, String> {
        let path = format!("/api/sessions/{id}{}", id_query(agent));
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            read_json::<SessionSummaryDto>(api.get(&path)?, "get session")
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn prompt(&self, id: &str, agent: Option<&str>, text: &str) -> Result<(), String> {
        let path = format!("/api/sessions/{id}/prompt{}", id_query(agent));
        let api = self.clone();
        let body = serde_json::json!({ "text": text });
        tokio::task::spawn_blocking(move || {
            let res = api.send_json("POST", &path, &body)?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "prompt"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), String> {
        let path = format!("/api/sessions/{id}/cancel{}", id_query(agent));
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            let res = api.send_json("POST", &path, &serde_json::json!({}))?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "cancel"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn list_agents(&self) -> Result<Vec<AgentDto>, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct AgentsBody {
                #[serde(default)]
                agents: Vec<AgentDto>,
            }
            read_json::<AgentsBody>(api.get("/api/agents")?, "list agents").map(|b| b.agents)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn list_projects(&self) -> Result<Vec<ProjectDto>, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct ProjectsBody {
                #[serde(default)]
                projects: Vec<ProjectDto>,
            }
            read_json::<ProjectsBody>(api.get("/api/projects")?, "list projects")
                .map(|b| b.projects)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn create_project(&self, name: &str, _node: Option<&str>) -> Result<ProjectDto, String> {
        let api = self.clone();
        let body = serde_json::json!({ "name": name });
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct CreatedBody {
                project: ProjectDto,
            }
            let res = api.send_json("POST", "/api/projects", &body)?;
            read_json::<CreatedBody>(res, "create project").map(|b| b.project)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn delete_project(&self, id: &str, _node: Option<&str>) -> Result<(), String> {
        let api = self.clone();
        let path = format!("/api/projects/{}", url_encode(id));
        tokio::task::spawn_blocking(move || {
            let res = api
                .request("DELETE", &path)
                .call()
                .map_err(|e| e.to_string())?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "delete project"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_config(&self) -> Result<BTreeMap<String, Value>, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct ConfigBody {
                #[serde(default)]
                config: BTreeMap<String, Value>,
            }
            read_json::<ConfigBody>(api.get("/api/config")?, "get config").map(|b| b.config)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn set_config(&self, key: &str, value: &Value) -> Result<(), String> {
        let api = self.clone();
        let path = format!("/api/config/{}", url_encode(key));
        let body = serde_json::json!({ "value": value });
        tokio::task::spawn_blocking(move || {
            let res = api.send_json("PATCH", &path, &body)?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "set config"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn node_info(&self) -> Result<NodeInfoDto, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            read_json::<NodeInfoDto>(api.get("/api/node")?, "get node")
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn node_status(&self) -> Result<Vec<NodeStatusDto>, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            // A direct connection sees exactly one node — probe it so a
            // down node still renders as a `down` row instead of an
            // erroring page.
            let row = match api.get("/api/node") {
                Ok(res) => match read_json::<NodeInfoDto>(res, "get node") {
                    Ok(info) => NodeStatusDto {
                        id: info.id,
                        url: api.base.clone(),
                        label: info.name,
                        status: "up".into(),
                        last_seen_at: None,
                    },
                    Err(_) => api.down_row(),
                },
                Err(_) => api.down_row(),
            };
            Ok(vec![row])
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn push_vapid(&self) -> Result<String, String> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            #[derive(serde::Deserialize)]
            struct VapidBody {
                #[serde(rename = "publicKey")]
                public_key: String,
            }
            read_json::<VapidBody>(api.get("/api/push/vapid")?, "get vapid key")
                .map(|b| b.public_key)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn push_subscribe(&self, subscription: &PushSubscriptionDto) -> Result<(), String> {
        let api = self.clone();
        let body = serde_json::to_value(subscription).map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || {
            let res = api.send_json("POST", "/api/push/subscribe", &body)?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "push subscribe"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn push_unsubscribe(&self, endpoint: &str) -> Result<(), String> {
        let api = self.clone();
        let body = serde_json::json!({ "endpoint": endpoint });
        tokio::task::spawn_blocking(move || {
            let res = api.delete_json("/api/push/subscribe", &body)?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "push unsubscribe"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
