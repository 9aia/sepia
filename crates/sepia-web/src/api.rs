//! Server functions — the browser never talks to the node directly.
//! Each `#[server]` fn runs on the hub (`ssr` builds: inline during SSR,
//! as an HTTP endpoint under `/hub/*` when called from wasm) and calls
//! the node API through the [`NodeApi`] port. `sepia-hub` provides the
//! client as context; unit tests substitute a stub.

// The `#[server]` macro's client-side trait impl generates `async` fns
// with no `.await` — not ours to fix.
#![allow(clippy::unused_async_trait_impl)]

use leptos::prelude::*;

use crate::dto::{HistoryPageDto, SessionSummaryDto};

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

    fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let url = format!("{}{}", self.base, path);
        let mut req = self.agent.post(&url);
        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        req.send_json(body).map_err(|e| e.to_string())
    }

    fn get(&self, path: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
        self.request("GET", path).call().map_err(|e| e.to_string())
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
            let res = api.post_json(&path, &body)?;
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
            let res = api.post_json(&path, &serde_json::json!({}))?;
            if res.status().is_success() {
                Ok(())
            } else {
                Err(error_of(res, "cancel"))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
