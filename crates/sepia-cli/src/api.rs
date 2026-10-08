//! Port of `apps/sepia/src/api-client.ts` — the HTTP client every
//! node verb runs through. One-shot JSON requests plus the SSE stream
//! reader; errors decode the server's `{error, code}` payload into
//! [`ApiError`] like the TS `toApiError`.

use std::io::Read;

use serde::Deserialize;
use serde_json::Value;

/// Where a node op lands — the CLI's analogue of the web client's
/// `ApiTarget`/`localTarget`. `SEPIA_NODE_URL`/`--node` repoints every
/// call, `SEPIA_TOKEN`/`--token` authenticates it.
#[derive(Clone, Debug)]
pub struct NodeTarget {
    pub base_url: String,
    pub token: Option<String>,
}

pub const DEFAULT_NODE_URL: &str = "http://127.0.0.1:8787";

pub fn default_node_url() -> String {
    std::env::var("SEPIA_NODE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_NODE_URL.to_string())
}

/// `token` falls back to `SEPIA_TOKEN`; `None` → unauthenticated calls.
pub fn resolve_target(node: &str, token: Option<&str>) -> NodeTarget {
    NodeTarget {
        base_url: node.trim_end_matches('/').to_string(),
        token: token
            .map(str::to_string)
            .or_else(|| std::env::var("SEPIA_TOKEN").ok()),
    }
}

/// A non-OK API response with the server's payload decoded — `status` is
/// the HTTP code, `code` the ControlError tag (`invalid`, `locked`,
/// `busy`…) when the body carried one. Mirrors `apps/web/src/lib/api.ts`.
#[derive(Debug)]
pub struct ApiError {
    pub message: String,
    pub status: Option<u16>,
    pub code: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

fn friendly_http_error(status: u16) -> String {
    match status {
        400 => "The server rejected the request".to_string(),
        401 => "Unauthorized — pass --token or set SEPIA_TOKEN".to_string(),
        403 => "Access denied".to_string(),
        404 => "Not found".to_string(),
        409 => "That operation is busy or locked — try again in a moment".to_string(),
        s if s >= 500 => "The server hit an error — try again".to_string(),
        s => format!("Request failed ({s})"),
    }
}

/// Prefer the server's `{error, code}` payload over generic status text.
fn to_api_error(status: u16, body: &str) -> ApiError {
    if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(body) {
        if let Some(error) = record.get("error").and_then(Value::as_str) {
            if !error.is_empty() {
                let code = record
                    .get("code")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string);
                return ApiError {
                    message: error.to_string(),
                    status: Some(status),
                    code,
                };
            }
        }
    }
    ApiError {
        message: friendly_http_error(status),
        status: Some(status),
        code: None,
    }
}

fn as_api_error(cause: impl std::fmt::Display, base_url: &str) -> ApiError {
    ApiError {
        message: format!("Cannot reach {base_url} — is a sepia node running there? ({cause})"),
        status: None,
        code: None,
    }
}

/// One agent for the process — `http_status_as_error(false)` so the
/// `{error, code}` body survives to `to_api_error` like fetch's `res.ok`.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build(),
        )
    })
}

fn urlencode(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// One JSON request against `target`, unwrapping the server's error
/// payload into `ApiError`. `body == None` sends no body (GET/DELETE).
pub fn request(
    target: &NodeTarget,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let url = format!("{}{}", target.base_url, path);
    let agent = agent();
    let res = match (method, body) {
        ("GET", None) => authorize(agent.get(&url), target).call(),
        ("DELETE", None) => authorize(agent.delete(&url), target).call(),
        (_, body) => {
            // Bodies on any method (incl. DELETE) — build the request
            // directly; the typed builders only cover GET/POST/PUT/PATCH.
            let mut builder = ureq::http::Request::builder().method(method).uri(&url);
            if let Some(token) = &target.token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            builder = builder.header("content-type", "application/json");
            let payload = serde_json::to_string(body.unwrap_or(&Value::Null)).unwrap_or_default();
            let request = builder
                .body(payload)
                .map_err(|e| as_api_error(e, &target.base_url))?;
            agent.run(request)
        }
    };
    let mut res = res.map_err(|e| as_api_error(e, &target.base_url))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| as_api_error(e, &target.base_url))?;
    if !res.status().is_success() {
        return Err(to_api_error(status, &text));
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| as_api_error(e, &target.base_url))
}

fn authorize<B>(req: ureq::RequestBuilder<B>, target: &NodeTarget) -> ureq::RequestBuilder<B> {
    match &target.token {
        Some(token) => req.header("authorization", format!("Bearer {token}")),
        None => req,
    }
}

/// Session ids collide across agents; `?agent=` scopes the lookup.
fn agent_query(agent: Option<&str>) -> String {
    match agent {
        Some(a) if !a.is_empty() => format!("?agent={}", urlencode(a)),
        _ => String::new(),
    }
}

/// `GET /api/fs` — list subdirectories of a path on the node.
pub fn list_dirs(target: &NodeTarget, path: &str) -> Result<Vec<String>, ApiError> {
    let data = request(
        target,
        "GET",
        &format!("/api/fs?path={}", urlencode(path)),
        None,
    )?;
    Ok(data
        .get("dirs")
        .and_then(Value::as_array)
        .map(|d| {
            d.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

// --- Sessions ---------------------------------------------------------------

/// The `GET /api/sessions` row (server's `SummaryWire` — meta overlay
/// fields are always present there, optional elsewhere).
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub agent: String,
    pub updated_at: String,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
}

/// The `GET /api/sessions/:id/history` page — kept as raw JSON so
/// `sessions resume` replays the full projection without dropping fields.
/// Callers read `messages[]`/`start`/`total` off the object.
pub type HistoryPage = Value;

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub agent_id: String,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub r#ref: String,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub run_count: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct RestoreResult {
    #[serde(default)]
    pub restored: Vec<RestoredFile>,
    #[serde(default)]
    pub skipped: Vec<SkippedFile>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct RestoredFile {
    pub path: String,
    pub action: String,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct RewindResult {
    pub kept: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    /// `capabilities` rides as an object once probed — presence matters,
    /// not contents.
    #[serde(default)]
    pub capabilities: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

pub fn get_health(target: &NodeTarget) -> Result<Value, ApiError> {
    request(target, "GET", "/api/health", None)
}

pub fn get_node(target: &NodeTarget) -> Result<Value, ApiError> {
    request(target, "GET", "/api/node", None)
}

pub fn get_user(target: &NodeTarget) -> Result<Value, ApiError> {
    request(target, "GET", "/api/user", None)
}

pub fn list_agents(target: &NodeTarget) -> Result<Vec<AgentInfo>, ApiError> {
    let data = request(target, "GET", "/api/agents", None)?;
    Ok(
        serde_json::from_value(data.get("agents").cloned().unwrap_or(Value::Null))
            .unwrap_or_default(),
    )
}

pub fn pair_redeem(target: &NodeTarget, code: &str) -> Result<String, ApiError> {
    let data = request(
        target,
        "POST",
        "/api/pair",
        Some(&serde_json::json!({ "code": code })),
    )?;
    Ok(data
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

pub fn list_sessions(
    target: &NodeTarget,
    with_locks: bool,
) -> Result<Vec<SessionSummary>, ApiError> {
    let path = if with_locks {
        "/api/sessions?withLocks=1"
    } else {
        "/api/sessions"
    };
    let data = request(target, "GET", path, None)?;
    serde_json::from_value(data.get("sessions").cloned().unwrap_or(Value::Null))
        .map_err(|e| as_api_error(e, &target.base_url))
}

pub fn create_session(target: &NodeTarget, input: &Value) -> Result<Value, ApiError> {
    request(target, "POST", "/api/sessions", Some(input))
}

pub fn get_history(
    target: &NodeTarget,
    id: &str,
    limit: Option<i64>,
    before: Option<i64>,
    agent: Option<&str>,
) -> Result<HistoryPage, ApiError> {
    let mut params = Vec::new();
    if let Some(limit) = limit {
        params.push(format!("limit={limit}"));
    }
    if let Some(before) = before {
        params.push(format!("before={before}"));
    }
    if let Some(agent) = agent {
        params.push(format!("agent={}", urlencode(agent)));
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let data = request(
        target,
        "GET",
        &format!("/api/sessions/{}/history{query}", urlencode(id)),
        None,
    )?;
    Ok(data)
}

pub fn get_checkpoints(
    target: &NodeTarget,
    id: &str,
    agent: Option<&str>,
) -> Result<Vec<Checkpoint>, ApiError> {
    let data = request(
        target,
        "GET",
        &format!(
            "/api/sessions/{}/checkpoints{}",
            urlencode(id),
            agent_query(agent)
        ),
        None,
    )?;
    serde_json::from_value(data.get("checkpoints").cloned().unwrap_or(Value::Null))
        .map_err(|e| as_api_error(e, &target.base_url))
}

/// The complete session IR — `{session}` on the wire, unwrapped here.
pub fn export_session(
    target: &NodeTarget,
    id: &str,
    agent: Option<&str>,
) -> Result<Value, ApiError> {
    let data = request(
        target,
        "GET",
        &format!(
            "/api/sessions/{}/export{}",
            urlencode(id),
            agent_query(agent)
        ),
        None,
    )?;
    Ok(data.get("session").cloned().unwrap_or(Value::Null))
}

pub fn attach(
    target: &NodeTarget,
    id: &str,
    takeover: bool,
    model: Option<&str>,
    fallbacks: Option<&[String]>,
    agent: Option<&str>,
) -> Result<AttachResult, ApiError> {
    let mut body = serde_json::Map::new();
    if takeover {
        body.insert("takeover".into(), Value::Bool(true));
    }
    if let Some(model) = model {
        body.insert("model".into(), Value::String(model.to_string()));
    }
    if let Some(fallbacks) = fallbacks {
        if !fallbacks.is_empty() {
            body.insert("fallbacks".into(), serde_json::json!(fallbacks));
        }
    }
    let data = request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/attach{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&Value::Object(body)),
    )?;
    serde_json::from_value(data).map_err(|e| as_api_error(e, &target.base_url))
}

pub fn prompt(
    target: &NodeTarget,
    id: &str,
    text: &str,
    agent: Option<&str>,
) -> Result<(), ApiError> {
    request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/prompt{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&serde_json::json!({ "text": text })),
    )?;
    Ok(())
}

pub fn cancel(target: &NodeTarget, id: &str, agent: Option<&str>) -> Result<(), ApiError> {
    request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/cancel{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&serde_json::json!({})),
    )?;
    Ok(())
}

pub fn respond_to_permission(
    target: &NodeTarget,
    id: &str,
    request_id: &str,
    option_id: Option<&str>,
    agent: Option<&str>,
) -> Result<(), ApiError> {
    request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/permission{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&serde_json::json!({ "requestId": request_id, "optionId": option_id })),
    )?;
    Ok(())
}

pub fn delete_session(target: &NodeTarget, id: &str, agent: Option<&str>) -> Result<(), ApiError> {
    request(
        target,
        "DELETE",
        &format!("/api/sessions/{}{}", urlencode(id), agent_query(agent)),
        None,
    )?;
    Ok(())
}

pub fn patch_session(
    target: &NodeTarget,
    id: &str,
    patch: &Value,
    agent: Option<&str>,
) -> Result<(), ApiError> {
    request(
        target,
        "PATCH",
        &format!("/api/sessions/{}{}", urlencode(id), agent_query(agent)),
        Some(patch),
    )?;
    Ok(())
}

pub fn convert_session(target: &NodeTarget, id: &str, to: &str) -> Result<String, ApiError> {
    let data = request(
        target,
        "POST",
        &format!("/api/sessions/{}/convert", urlencode(id)),
        Some(&serde_json::json!({ "agent": to })),
    )?;
    Ok(data
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

pub fn import_session(target: &NodeTarget, input: &Value) -> Result<SessionSummary, ApiError> {
    let data = request(target, "POST", "/api/sessions/import", Some(input))?;
    serde_json::from_value(data).map_err(|e| as_api_error(e, &target.base_url))
}

pub fn restore_session(
    target: &NodeTarget,
    id: &str,
    selector: &Value,
    agent: Option<&str>,
) -> Result<RestoreResult, ApiError> {
    let mut body = selector.as_object().cloned().unwrap_or_default();
    body.insert("confirm".into(), Value::Bool(true));
    let data = request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/restore{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&Value::Object(body)),
    )?;
    serde_json::from_value(data).map_err(|e| as_api_error(e, &target.base_url))
}

pub fn rewind_session(
    target: &NodeTarget,
    id: &str,
    selector: &Value,
    agent: Option<&str>,
) -> Result<RewindResult, ApiError> {
    let mut body = selector.as_object().cloned().unwrap_or_default();
    body.insert("confirm".into(), Value::Bool(true));
    let data = request(
        target,
        "POST",
        &format!(
            "/api/sessions/{}/rewind{}",
            urlencode(id),
            agent_query(agent)
        ),
        Some(&Value::Object(body)),
    )?;
    serde_json::from_value(data).map_err(|e| as_api_error(e, &target.base_url))
}

// --- Meta / config / projects ------------------------------------------------

pub fn get_config(target: &NodeTarget) -> Result<Value, ApiError> {
    let data = request(target, "GET", "/api/config", None)?;
    Ok(data.get("config").cloned().unwrap_or(Value::Null))
}

pub fn set_config(target: &NodeTarget, key: &str, value: &Value) -> Result<(), ApiError> {
    request(
        target,
        "PATCH",
        &format!("/api/config/{}", urlencode(key)),
        Some(&serde_json::json!({ "value": value })),
    )?;
    Ok(())
}

pub fn list_projects(target: &NodeTarget) -> Result<Vec<Project>, ApiError> {
    let data = request(target, "GET", "/api/projects", None)?;
    serde_json::from_value(data.get("projects").cloned().unwrap_or(Value::Null))
        .map_err(|e| as_api_error(e, &target.base_url))
}

pub fn create_project(target: &NodeTarget, name: &str) -> Result<Project, ApiError> {
    let data = request(
        target,
        "POST",
        "/api/projects",
        Some(&serde_json::json!({ "name": name })),
    )?;
    serde_json::from_value(data.get("project").cloned().unwrap_or(Value::Null))
        .map_err(|e| as_api_error(e, &target.base_url))
}

pub fn rename_project(target: &NodeTarget, id: &str, name: &str) -> Result<(), ApiError> {
    request(
        target,
        "PATCH",
        &format!("/api/projects/{}", urlencode(id)),
        Some(&serde_json::json!({ "name": name })),
    )?;
    Ok(())
}

pub fn delete_project(target: &NodeTarget, id: &str) -> Result<(), ApiError> {
    request(
        target,
        "DELETE",
        &format!("/api/projects/{}", urlencode(id)),
        None,
    )?;
    Ok(())
}

// --- Push ---------------------------------------------------------------------

pub fn get_vapid(target: &NodeTarget) -> Result<String, ApiError> {
    let data = request(target, "GET", "/api/push/vapid", None)?;
    Ok(data
        .get("publicKey")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

pub fn push_subscribe(target: &NodeTarget, subscription: &Value) -> Result<(), ApiError> {
    request(target, "POST", "/api/push/subscribe", Some(subscription))?;
    Ok(())
}

pub fn push_unsubscribe(target: &NodeTarget, endpoint: &str) -> Result<(), ApiError> {
    request(
        target,
        "DELETE",
        "/api/push/subscribe",
        Some(&serde_json::json!({ "endpoint": endpoint })),
    )?;
    Ok(())
}

// --- SSE ----------------------------------------------------------------------

/// One parsed SSE frame — the `event:` name (absent on unnamed frames)
/// plus the joined `data:` lines.
#[derive(Clone, Debug, PartialEq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

fn parse_frame(raw: &str) -> Option<SseFrame> {
    let mut event: Option<String> = None;
    let mut data: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.trim().to_string());
        }
    }
    if data.is_empty() && event.is_none() {
        None
    } else {
        Some(SseFrame {
            event,
            data: data.join("\n"),
        })
    }
}

/// Send an SSE request and invoke `on_frame` per frame until the server
/// closes the stream. `?access_token` is the server's documented auth
/// path for the two GET streams, but the bearer header works for HTTP
/// clients and is what we send.
pub fn stream_sse(
    target: &NodeTarget,
    method: &str,
    path: &str,
    body: Option<&Value>,
    mut on_frame: impl FnMut(&SseFrame),
) -> Result<(), ApiError> {
    let url = format!("{}{}", target.base_url, path);
    let res = match (method, body) {
        ("GET", None) => authorize(agent().get(&url), target)
            .header("accept", "text/event-stream")
            .call(),
        (_, body) => authorize(agent().post(&url), target)
            .header("accept", "text/event-stream")
            .header("content-type", "application/json")
            .send_json(body.unwrap_or(&Value::Null)),
    };
    let res = res.map_err(|e| as_api_error(e, &target.base_url))?;
    if !res.status().is_success() {
        let status = res.status().as_u16();
        let mut body = res.into_body();
        let text = body.read_to_string().unwrap_or_default();
        return Err(to_api_error(status, &text));
    }
    let mut reader = res.into_body().into_reader();
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|e| as_api_error(e, &target.base_url))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        // Frame boundaries are ASCII "\n\n" — splitting at byte level
        // never straddles a multibyte char.
        while let Some(at) = find_subslice(&buffer, b"\n\n") {
            let raw: Vec<u8> = buffer.drain(..at).collect();
            buffer.drain(..2);
            let text = String::from_utf8_lossy(&raw);
            if let Some(frame) = parse_frame(&text) {
                on_frame(&frame);
            }
        }
    }
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
