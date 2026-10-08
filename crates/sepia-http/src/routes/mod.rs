//! Route helpers — port of `apps/server/src/routes/shared.ts`: the
//! `ControlError` → HTTP status mapping, the `{error, code?}` body shape,
//! JSON-body reading, and the wire structs the handlers serialize.

pub mod agents;
pub mod client;
pub mod config;
pub mod events;
pub mod misc;
pub mod projects;
pub mod sessions;

use std::collections::HashMap;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sepia_acp::{AcpCapabilities, AcpPromptCapabilities, AcpSessionCapabilities};
use sepia_control::{ControlError, ControlErrorCode, SessionSummary};
use sepia_meta::SessionMeta;
use serde::Serialize;
use serde_json::Value;

/// `CODE_STATUS` — the wire taxonomy → HTTP status.
pub fn code_status(code: ControlErrorCode) -> StatusCode {
    match code {
        ControlErrorCode::NotFound => StatusCode::NOT_FOUND,
        ControlErrorCode::Invalid | ControlErrorCode::UnknownAgent => StatusCode::BAD_REQUEST,
        ControlErrorCode::Locked | ControlErrorCode::Conflict | ControlErrorCode::Busy => {
            StatusCode::CONFLICT
        }
        ControlErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `{error: message, code?: "snake_case"}` — the flat TS body shape.
pub fn json_response(body: Value, status: StatusCode) -> Response {
    (status, axum::Json(body)).into_response()
}

/// TS `errorResponse` — a coded failure maps via `CODE_STATUS`; anything
/// else is a bare `{error}` 500.
pub fn error_response(error: &ControlError) -> Response {
    let status = code_status(error.code);
    json_response(
        serde_json::json!({ "error": error.message, "code": error.code.as_str() }),
        status,
    )
}

/// `respond(run, effect, …)` — the uniform tail of every plane call.
pub fn respond<T: Serialize>(result: Result<T, ControlError>, status: StatusCode) -> Response {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(v) => json_response(v, status),
            Err(e) => json_response(
                serde_json::json!({ "error": e.to_string() }),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        },
        Err(error) => {
            tracing::warn!("request failed: {error}");
            error_response(&error)
        }
    }
}

pub fn ok() -> Response {
    json_response(serde_json::json!({ "ok": true }), StatusCode::OK)
}

/// Terminal handler — TS falls through the whole route table to
/// `{error:"Not found"}` 404.
pub fn not_found() -> Response {
    json_response(
        serde_json::json!({ "error": "Not found" }),
        StatusCode::NOT_FOUND,
    )
}

/// `readJsonBody` — empty body → `None` (TS `undefined`); parse failure
/// → the boxed 400 `Invalid JSON body` response (`Box` keeps the `Err`
/// variant small for `clippy::result_large_err`).
pub async fn read_json_body(req: Request<Body>) -> Result<Option<Value>, Box<Response>> {
    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|_| Box::new(invalid_json()))?;
    read_json_bytes(&bytes)
}

fn invalid_json() -> Response {
    json_response(
        serde_json::json!({ "error": "Invalid JSON body" }),
        StatusCode::BAD_REQUEST,
    )
}

pub fn read_json_bytes(bytes: &Bytes) -> Result<Option<Value>, Box<Response>> {
    let text = String::from_utf8_lossy(bytes);
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Value>(text.trim())
        .map(Some)
        .map_err(|_| Box::new(invalid_json()))
}

/// `url.searchParams` — a decoded name→value map.
pub fn query_params(query: Option<&str>) -> HashMap<String, String> {
    let Some(query) = query else {
        return HashMap::new();
    };
    form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    })
}

/// `?agent` — session ids collide across agent stores, so session-scoped
/// routes accept it to scope the store lookup.
pub fn agent_param(req: &Request) -> Option<String> {
    query_param(req.uri().query(), "agent").filter(|a| !a.is_empty())
}

/// `Date.now()` — epoch milliseconds.
pub fn now_ms_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// `new Date().toISOString()` — milliseconds, `Z` suffix.
pub fn iso_now() -> String {
    iso_of_ms(now_ms_f64())
}

pub fn iso_of_ms(ms: f64) -> String {
    let nanos = (ms * 1e6) as i128;
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp_nanos(nanos) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond(),
    )
}

/// ISO 8601 from epoch **seconds** (`Session.last_activity_at`).
pub fn iso_of_secs(secs: f64) -> String {
    iso_of_ms(secs * 1000.0)
}

/* ---- wire shapes ----------------------------------------------------*/

/// `AcpCapabilities` under its TS field names — `loadSession`,
/// `sessionList`, `promptCapabilities`, `sessionCapabilities`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesWire {
    pub load_session: bool,
    pub session_list: bool,
    pub prompt_capabilities: AcpPromptCapabilities,
    pub session_capabilities: AcpSessionCapabilities,
}

impl From<&AcpCapabilities> for CapabilitiesWire {
    fn from(c: &AcpCapabilities) -> Self {
        Self {
            load_session: c.load_session,
            session_list: c.session_list,
            prompt_capabilities: c.prompt_capabilities.clone(),
            session_capabilities: c.session_capabilities.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResultWire {
    pub attached: bool,
    pub read_only: bool,
    pub agent_id: String,
    pub capabilities: CapabilitiesWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResultWire {
    pub id: String,
    pub agent_id: String,
    pub capabilities: CapabilitiesWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageWire {
    pub messages: Vec<sepia_control::HistoryMessage>,
    pub total: usize,
    pub start: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWire {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilitiesWire>,
}

/// The `GET /api/sessions` row: `SessionSummary` with the meta overlay
/// applied — `pinned`/`archived`/`projectIds`/`model`/`spans` always
/// ride, defaulting like the TS `meta?.x ?? fallback` merge.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryWire {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub agent: String,
    pub updated_at: String,
    pub locked: bool,
    pub lock_holder_pid: Option<f64>,
    pub source: String,
    pub busy: bool,
    pub pinned: bool,
    pub archived: bool,
    pub project_ids: Vec<String>,
    pub model: Option<String>,
    pub spans: Vec<sepia_meta::RunSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl SummaryWire {
    /// `sessions.map` in the TS list handler: overlay fields come from
    /// the meta record when present, else the defaults.
    pub fn overlay(session: SessionSummary, meta: Option<&SessionMeta>) -> Self {
        Self {
            id: session.id,
            title: meta.and_then(|m| m.title.clone()).unwrap_or(session.title),
            cwd: session.cwd,
            agent: session.agent,
            updated_at: session.updated_at,
            locked: session.locked,
            lock_holder_pid: session.lock_holder_pid,
            source: session.source,
            busy: session.busy,
            pinned: meta.and_then(|m| m.pinned).unwrap_or(false),
            archived: meta.and_then(|m| m.archived).unwrap_or(false),
            project_ids: meta.and_then(|m| m.project_ids.clone()).unwrap_or_default(),
            model: meta.and_then(|m| m.model.clone()).flatten(),
            spans: meta.and_then(|m| m.spans.clone()).unwrap_or_default(),
            parent_session_id: session.parent_session_id,
            agent_id: session.agent_id,
        }
    }

    /// A created-but-unflushed session — survives restarts only in the
    /// meta file, so the list surfaces it with recorded agent/cwd.
    pub fn pending(id: &str, meta: &SessionMeta) -> Self {
        Self {
            id: id.to_string(),
            title: meta.title.clone().unwrap_or_else(|| "New session".into()),
            cwd: meta.cwd.clone().unwrap_or_default(),
            agent: meta.agent.clone().unwrap_or_default(),
            updated_at: meta.created_at.clone().unwrap_or_else(iso_now),
            locked: false,
            lock_holder_pid: None,
            source: "sepia".into(),
            busy: false,
            pinned: meta.pinned.unwrap_or(false),
            archived: meta.archived.unwrap_or(false),
            project_ids: meta.project_ids.clone().unwrap_or_default(),
            model: meta.model.clone().flatten(),
            spans: meta.spans.clone().unwrap_or_default(),
            parent_session_id: None,
            agent_id: None,
        }
    }
}

pub mod push;
