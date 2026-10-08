//! `/api/sessions*` — port of `apps/server/src/routes/sessions.ts`: the
//! whole session surface — collection list/create, `import`, the meta
//! `PATCH`, `convert`, `DELETE`, and the `:id/:action` verbs (history,
//! checkpoints, export, stream, attach, detach, prompt, restore, rewind,
//! cancel, permission).

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use futures::stream;
use sepia_acp::PromptPart;
use sepia_control::{
    ControlError, ControlErrorCode, HistoryOptions, RestoreRequest, RewindRequest,
};
use sepia_core::Session;
use sepia_core::wire::session_to_json;
use sepia_meta::{RunSpan, SessionMeta};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::AppState;
use crate::routes::{
    AttachResultWire, CreateResultWire, HistoryPageWire, SummaryWire, agent_param, error_response,
    iso_now, iso_of_secs, json_response, not_found, now_ms_f64, ok, query_param, read_json_body,
};

/// `POST /api/sessions/import` target store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportTarget {
    Cline,
    Devin,
}

impl ImportTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cline => "cline",
            Self::Devin => "devin",
        }
    }
}

/// The store-write seam for `POST /api/sessions/import` — the TS
/// `importSession` option. Absent → the route answers 501.
pub type ImportSession = Arc<
    dyn Fn(Session, ImportTarget) -> BoxFuture<'static, Result<String, ControlError>> + Send + Sync,
>;

/// The store-write seam for `POST /api/sessions/:id/convert` — absent →
/// the route answers 501.
pub type ConvertSession = Arc<
    dyn Fn(String, ImportTarget) -> BoxFuture<'static, Result<String, ControlError>> + Send + Sync,
>;

fn bad_request(message: &str) -> Response {
    json_response(json!({ "error": message }), StatusCode::BAD_REQUEST)
}

fn not_configured(feature: &str) -> Response {
    json_response(
        json!({ "error": format!("{feature} is not configured on this server") }),
        StatusCode::NOT_IMPLEMENTED,
    )
}

fn respond_map<T>(
    result: Result<T, ControlError>,
    status: StatusCode,
    shape: impl FnOnce(T) -> Value,
) -> Response {
    match result {
        Ok(v) => json_response(shape(v), status),
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
    }
}

fn meta_of(state: &AppState, id: &str) -> Option<SessionMeta> {
    state.meta.as_ref().and_then(|m| m.of(id))
}

/* ---- collection -----------------------------------------------------*/

/// `GET|POST /api/sessions`.
pub async fn collection(
    State(state): State<AppState>,
    method: Method,
    req: Request<Body>,
) -> Response {
    let with_locks = query_param(req.uri().query(), "withLocks").as_deref() == Some("1");
    match method {
        Method::GET => list(&state, with_locks).await,
        Method::POST => create(&state, req).await,
        _ => not_found(),
    }
}

/// `GET /api/sessions` — store summaries + meta overlay + meta-only
/// pending rows (created-but-unflushed sessions).
async fn list(state: &AppState, with_locks: bool) -> Response {
    let result = state.plane.list_sessions(with_locks).await;
    respond_map(result, StatusCode::OK, |sessions| {
        let mut overlaid: Vec<SummaryWire> = Vec::with_capacity(sessions.len());
        let mut known = std::collections::HashSet::new();
        for session in sessions {
            let meta = meta_of(state, &session.id);
            known.insert(session.id.clone());
            overlaid.push(SummaryWire::overlay(session, meta.as_ref()));
        }
        // Sessions created via POST /api/sessions but not yet flushed
        // into the agent's store survive restarts only in the meta
        // file — surface them so they stay reachable.
        let pending: Vec<SummaryWire> = state
            .meta
            .as_ref()
            .map(crate::feed::InstrumentedMeta::sessions)
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, meta)| !known.contains(id) && meta.agent.is_some() && meta.cwd.is_some())
            .map(|(id, meta)| SummaryWire::pending(&id, &meta))
            .collect();
        let mut all = overlaid;
        all.extend(pending);
        json!({ "sessions": all })
    })
}

/// `POST /api/sessions` — create a session under an agent runtime.
async fn create(state: &AppState, req: Request<Body>) -> Response {
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body else {
        return bad_request("Expected a JSON object body");
    };
    let Some(body) = body.as_object() else {
        return bad_request("Expected a JSON object body");
    };
    let cwd = body.get("cwd");
    match cwd {
        Some(Value::String(c)) if !c.trim().is_empty() => {}
        _ => return bad_request("cwd is required"),
    }
    let cwd = cwd.and_then(Value::as_str).unwrap_or_default().to_string();
    let agent_id = match body.get("agent") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("agent must be a string"),
    };
    let title = match body.get("title") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("title must be a string"),
    };
    let model = match body.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("model must be a string"),
    };
    let fallbacks = match body.get("fallbacks") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => Some(
            items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        ),
        _ => return bad_request("fallbacks must be an array of strings"),
    };

    match state
        .plane
        .create_session(&cwd, agent_id, title.clone(), model.clone(), fallbacks)
        .await
    {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(created) => {
            // The agent may not flush the session to its store until the
            // first prompt; keep enough meta to identify it after a
            // restart.
            let mut patch = json!({ "created": true, "cwd": cwd });
            if let Some(t) = &title {
                patch["title"] = json!(t);
            }
            state
                .feed
                .emit_session(&created.id, Some(&created.agent_id), patch);
            state.live.register(
                &state.plane,
                &state.feed,
                &created.id,
                Some(created.agent_id.clone()),
            );
            if let Some(meta) = &state.meta {
                meta.patch(
                    &created.id,
                    &SessionMeta {
                        agent: Some(created.agent_id.clone()),
                        cwd: Some(cwd),
                        created_at: Some(iso_now()),
                        model: model.map(Some),
                        title,
                        ..SessionMeta::default()
                    },
                );
            }
            json_response(
                json!(CreateResultWire {
                    id: created.id,
                    agent_id: created.agent_id,
                    capabilities: (&created.capabilities).into(),
                }),
                StatusCode::CREATED,
            )
        }
    }
}

/* ---- item -----------------------------------------------------------*/

/// `GET|PATCH|DELETE /api/sessions/{id}` plus the `/{id}/meta` alias for
/// the meta PATCH.
pub async fn item(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    let agent = agent_param(&req);
    match method {
        Method::GET => get_one(&state, &id, agent.as_deref()).await,
        Method::PATCH => meta_patch(&state, &id, req).await,
        Method::DELETE => delete(&state, &id, agent.as_deref()).await,
        _ => not_found(),
    }
}

/// `PATCH /api/sessions/{id}/meta` — alias of the item PATCH.
pub async fn meta_alias(
    state: State<AppState>,
    method: Method,
    path: Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::PATCH {
        return not_found();
    }
    item(state, method, path, req).await
}

/// `GET /api/sessions/{id}` — the summary row for one session (meta
/// overlay applied, same shape as the list rows).
async fn get_one(state: &AppState, id: &str, agent: Option<&str>) -> Response {
    let result = state.plane.get_summary(id, agent).await;
    respond_map(result, StatusCode::OK, |session| {
        let summary = sepia_control::SessionSummary {
            id: session.id.clone(),
            title: session.title.clone(),
            cwd: session.working_directory.clone(),
            agent: sepia_control::agent_for_backend(&session.backend_type).to_string(),
            updated_at: iso_of_secs(session.last_activity_at),
            locked: false,
            lock_holder_pid: None,
            source: sepia_control::agent_for_backend(&session.backend_type).to_string(),
            busy: false,
            pinned: None,
            archived: None,
            project_ids: None,
            spans: None,
            parent_session_id: session.parent_session_id.clone(),
            agent_id: session.agent_id.clone(),
        };
        let meta = meta_of(state, &session.id);
        json!(SummaryWire::overlay(summary, meta.as_ref()))
    })
}

/// `PATCH /api/sessions/{id}` — title/pin/archive/project/model overlay.
async fn meta_patch(state: &AppState, id: &str, req: Request<Body>) -> Response {
    let Some(meta) = &state.meta else {
        return not_configured("Rename");
    };
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body.and_then(|b| b.as_object().cloned()) else {
        return bad_request("Expected a JSON object body");
    };
    let mut patch = SessionMeta::default();
    if let Some(title) = body.get("title") {
        match title
            .as_str()
            .filter(|t| !t.trim().is_empty() && t.len() <= 200)
        {
            Some(t) => patch.title = Some(t.trim().to_string()),
            None => return bad_request("title must be a non-empty string (max 200)"),
        }
    }
    if let Some(pinned) = body.get("pinned") {
        match pinned.as_bool() {
            Some(b) => patch.pinned = Some(b),
            None => return bad_request("pinned must be a boolean"),
        }
    }
    if let Some(archived) = body.get("archived") {
        match archived.as_bool() {
            Some(b) => patch.archived = Some(b),
            None => return bad_request("archived must be a boolean"),
        }
    }
    if let Some(project_ids) = body.get("projectIds") {
        match project_ids.as_array() {
            Some(ids) if ids.iter().all(Value::is_string) => {
                patch.project_ids = Some(
                    ids.iter()
                        .filter_map(|i| i.as_str().map(str::to_string))
                        .collect(),
                );
            }
            _ => return bad_request("projectIds must be an array of strings"),
        }
    }
    if let Some(model) = body.get("model") {
        match model {
            Value::Null => patch.model = Some(None),
            Value::String(s) if s.len() <= 100 => patch.model = Some(Some(s.clone())),
            _ => return bad_request("model must be a string or null"),
        }
    }
    if patch == SessionMeta::default() {
        return bad_request("Nothing to patch");
    }
    meta.patch(id, &patch);
    ok()
}

/// `DELETE /api/sessions/{id}` — a created-but-unflushed session isn't
/// in the repo; deleting it is still a success.
async fn delete(state: &AppState, id: &str, agent: Option<&str>) -> Response {
    let result = match state.plane.delete_session(id, agent).await {
        Ok(()) => Ok(()),
        Err(e) if e.code == ControlErrorCode::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        tracing::warn!("request failed: {e}");
        return error_response(&e);
    }
    state.held.unwatch(id, agent);
    state
        .feed
        .emit_session(id, agent, json!({ "deleted": true }));
    if let Some(meta) = &state.meta {
        meta.remove(id);
    }
    ok()
}

/* ---- GET actions ----------------------------------------------------*/

fn history_param(req: &Request<Body>, key: &str) -> Result<Option<f64>, Box<Response>> {
    match query_param(req.uri().query(), key) {
        None => Ok(None),
        Some(raw) if raw.is_empty() => Ok(None),
        Some(raw) => match raw.parse::<f64>() {
            Ok(v) if v.is_finite() && v.fract() == 0.0 && v >= 0.0 => Ok(Some(v)),
            _ => Err(Box::new(bad_request(&format!(
                "{key} must be a non-negative integer"
            )))),
        },
    }
}

/// `GET /api/sessions/{id}/history` — `?limit`/`?before` paging.
pub async fn history(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agent = agent_param(&req);
    let limit = match history_param(&req, "limit") {
        Ok(v) => v.map(|v| v as usize),
        Err(res) => return *res,
    };
    let before = match history_param(&req, "before") {
        Ok(v) => v.map(|v| v as i64),
        Err(res) => return *res,
    };
    let options = HistoryOptions {
        limit,
        before,
        agent_id: agent,
    };
    let result = state.plane.get_history(&id, &options).await;
    respond_map(result, StatusCode::OK, |page| {
        json!(HistoryPageWire {
            messages: page.messages,
            total: page.total,
            start: page.start,
        })
    })
}

/// `GET /api/sessions/{id}/checkpoints` — the workspace snapshot refs.
pub async fn checkpoints(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agent = agent_param(&req);
    let result = state.plane.get_summary(&id, agent.as_deref()).await;
    respond_map(
        result,
        StatusCode::OK,
        |session| json!({ "checkpoints": session.checkpoints }),
    )
}

/// `GET /api/sessions/{id}/export` — the complete session IR, consumed
/// by a peer node's `/import` for a lossless cross-node resume.
pub async fn export(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agent = agent_param(&req);
    match state.plane.get_session(&id, agent.as_deref()).await {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(session) => match session_to_json(&session) {
            Ok(v) => json_response(json!({ "session": v }), StatusCode::OK),
            Err(e) => json_response(
                json!({ "error": e.to_string() }),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        },
    }
}

/* ---- stream ---------------------------------------------------------*/

/// The lag marker emitted when a subscriber falls behind the broadcast
/// ring — clients refetch on reconnect/resync anyway.
fn lagged_event() -> Event {
    Event::default().event("lagged").data("{}")
}

/// `GET /api/sessions/{id}/stream` — subscribe() events serialized as
/// `data: {json}` frames plus keep-alive comments on the configured
/// cadence (`0` disables).
pub async fn stream_events(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::GET {
        return not_found();
    }
    let agent = agent_param(&req);
    let rx = match state.plane.subscribe(&id, agent.as_deref()).await {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            return error_response(&e);
        }
        Ok(rx) => rx,
    };
    // The broadcast receiver fans out into an mpsc so a client that
    // stops draining terminates the stream instead of back-pressuring
    // the session's event fan-out (the TS SseChannel outstanding-byte
    // cap plays the same role).
    let (tx, mpsc_rx) = tokio::sync::mpsc::channel::<Event>(256);
    tokio::spawn(async move {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(events) => {
                    for event in &events {
                        let data =
                            serde_json::to_string(event).unwrap_or_else(|_| "null".to_string());
                        // Dropping the receiver ends the stream — the
                        // response body was cancelled by the client.
                        if tx.try_send(Event::default().data(data)).is_err() {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if tx.try_send(lagged_event()).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    let stream = stream::unfold(mpsc_rx, |mut rx| async move {
        rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx))
    });
    let sse = Sse::new(stream);
    let mut res = if state.keep_alive > std::time::Duration::ZERO {
        sse.keep_alive(KeepAlive::new().interval(state.keep_alive).text("ping"))
            .into_response()
    } else {
        sse.into_response()
    };
    // TS `sseHeaders` also pins `Connection: keep-alive`.
    res.headers_mut().insert(
        axum::http::header::CONNECTION,
        axum::http::HeaderValue::from_static("keep-alive"),
    );
    res
}

/* ---- POST actions ---------------------------------------------------*/

/// `POST /api/sessions/{id}/attach` — `{takeover?, model?, fallbacks?}`.
pub async fn attach(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let mut takeover = false;
    let mut model = None;
    let mut fallbacks = None;
    if let Some(body) = body {
        let Some(body) = body.as_object() else {
            return bad_request("Expected a JSON object body");
        };
        takeover = body.get("takeover") == Some(&Value::Bool(true));
        if let Some(Value::String(m)) = body.get("model") {
            model = Some(m.clone());
        }
        if let Some(Value::Array(items)) = body.get("fallbacks") {
            if items.iter().all(Value::is_string) {
                fallbacks = Some(
                    items
                        .iter()
                        .filter_map(|i| i.as_str().map(str::to_string))
                        .collect::<Vec<_>>(),
                );
            }
        }
    }
    match state
        .plane
        .attach(&id, takeover, model, fallbacks, agent.clone())
        .await
    {
        Err(e) => {
            // A failed takeover leaves the session held — keep the watch
            // so the release edge still reaches the feed.
            if e.code == ControlErrorCode::Locked {
                state.held.watch(&id, agent.as_deref());
            }
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(result) => {
            state
                .live
                .register(&state.plane, &state.feed, &id, agent.clone());
            state.feed.emit_session(
                &id,
                Some(&result.agent_id),
                json!({ "live": result.attached }),
            );
            // Read-only means another process holds the store lock — the
            // watch turns its release into feed events so held clients
            // never poll for it.
            if result.read_only {
                state.held.watch(&id, Some(&result.agent_id));
            } else if result.attached {
                state.held.unwatch(&id, Some(&result.agent_id));
            }
            // Provenance: an attach means the run continues under this
            // node's control plane — record which agent + node own the
            // span. Idempotent, so a same-agent re-attach doesn't dup.
            if result.attached {
                if let Some(meta) = &state.meta {
                    meta.add_span(
                        &id,
                        RunSpan {
                            at: now_ms_f64(),
                            agent: result.agent_id.clone(),
                            node: state.node.id.clone(),
                        },
                    );
                }
            }
            json_response(
                json!(AttachResultWire {
                    attached: result.attached,
                    read_only: result.read_only,
                    agent_id: result.agent_id,
                    capabilities: (&result.capabilities).into(),
                }),
                StatusCode::OK,
            )
        }
    }
}

/// `POST /api/sessions/{id}/detach` — release the live attach (no-op
/// when not attached).
pub async fn detach(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    _req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    state.plane.detach(&id).await;
    ok()
}

/* ---- prompt ---------------------------------------------------------*/

/// Prompt payload caps — roughly the web composer's 5MB budget after
/// base64 inflation.
const MAX_PROMPT_PARTS: usize = 16;
const MAX_PROMPT_PART_CHARS: usize = 8 * 1024 * 1024;

/// Structural guard for one ACP `session/prompt` content block — the
/// `PromptPart` union. Anything else rejects the whole request: a
/// silently dropped part would make the agent see a prompt the user
/// didn't send.
fn is_prompt_part(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let Some(kind) = obj.get("type").and_then(Value::as_str) else {
        return false;
    };
    let non_empty = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    match kind {
        "text" => obj.get("text").is_some_and(Value::is_string),
        "image" | "audio" => non_empty(obj.get("data")) && non_empty(obj.get("mimeType")),
        "resource" => {
            let Some(resource) = obj.get("resource").and_then(Value::as_object) else {
                return false;
            };
            if !non_empty(resource.get("uri")) {
                return false;
            }
            let has_text = resource.get("text").is_some_and(Value::is_string);
            let has_blob = resource.get("blob").is_some_and(Value::is_string);
            let mime_ok = resource.get("mimeType").is_none_or(Value::is_string);
            (has_text || has_blob) && mime_ok
        }
        "resource_link" => {
            non_empty(obj.get("uri"))
                && non_empty(obj.get("name"))
                && obj.get("mimeType").is_none_or(Value::is_string)
                && obj
                    .get("size")
                    .is_none_or(|s| s.as_f64().is_some_and(|n| n >= 0.0))
        }
        _ => false,
    }
}

/// UTF-16-ish length (`chars().count`) — close enough to JS
/// `string.length` for the payload cap.
fn payload_chars(part: &Value) -> usize {
    let Some(obj) = part.as_object() else {
        return 0;
    };
    let count = |v: Option<&Value>| v.and_then(Value::as_str).map_or(0, |s| s.chars().count());
    match obj.get("type").and_then(Value::as_str) {
        Some("text") => count(obj.get("text")),
        Some("image" | "audio") => count(obj.get("data")),
        Some("resource") => obj
            .get("resource")
            .and_then(Value::as_object)
            .map_or(0, |r| count(r.get("text").or_else(|| r.get("blob")))),
        _ => 0,
    }
}

/// `{text, attachments?}` → the content-block list handed to the agent:
/// text first, attachments in send order. `Err` carries the 400 message.
fn prompt_parts_from_body(body: Option<Value>) -> Result<Vec<PromptPart>, String> {
    let Some(body) = body else {
        return Err("Expected a JSON object body".to_string());
    };
    let Some(body) = body.as_object() else {
        return Err("Expected a JSON object body".to_string());
    };
    let text = match body.get("text") {
        None => None,
        Some(Value::String(t)) => Some(t.clone()),
        _ => return Err("text must be a string".to_string()),
    };
    let attachments = match body.get("attachments") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        _ => return Err("attachments must be an array of content blocks".to_string()),
    };
    if attachments.len() > MAX_PROMPT_PARTS {
        return Err(format!("Too many attachments — max {MAX_PROMPT_PARTS}"));
    }
    let mut parts: Vec<PromptPart> = Vec::new();
    if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
        parts.push(PromptPart::Text { text: t });
    }
    let mut chars = 0usize;
    for item in &attachments {
        if !is_prompt_part(item) {
            return Err(
                "attachments must be ACP content blocks (text, image, audio, resource, resource_link)"
                    .to_string(),
            );
        }
        chars += payload_chars(item);
        if chars > MAX_PROMPT_PART_CHARS {
            return Err("Attachments exceed the size limit".to_string());
        }
        // Structurally validated — deserialization cannot fail.
        match serde_json::from_value::<PromptPart>(item.clone()) {
            Ok(part) => parts.push(part),
            Err(_) => {
                return Err(
                    "attachments must be ACP content blocks (text, image, audio, resource, resource_link)"
                        .to_string(),
                );
            }
        }
    }
    if parts.is_empty() {
        return Err("text or attachments is required".to_string());
    }
    Ok(parts)
}

/// `POST /api/sessions/{id}/prompt` — `{text?, attachments?}`.
pub async fn prompt(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let parts = match prompt_parts_from_body(body) {
        Ok(parts) => parts,
        Err(message) => return bad_request(&message),
    };
    match state.plane.prompt(&id, &parts, agent.as_deref()).await {
        Ok(()) => ok(),
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
    }
}

/// `POST /api/sessions/{id}/cancel` — fire-and-forget turn cancel.
pub async fn cancel(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    match state.plane.cancel(&id, agent.as_deref()).await {
        Ok(()) => ok(),
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
    }
}

/// `POST /api/sessions/{id}/permission` (also `/permissions`) — settle a
/// pending permission request. `{requestId, optionId?}` where `optionId`
/// may be `null`.
pub async fn permission(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body.and_then(|b| b.as_object().cloned()) else {
        return bad_request("requestId is required");
    };
    let Some(request_id) = body.get("requestId").and_then(Value::as_str) else {
        return bad_request("requestId is required");
    };
    let request_id = request_id.to_string();
    let option_id = match body.get("optionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("optionId must be a string or null"),
    };
    match state
        .plane
        .respond_to_permission(&id, &request_id, option_id.as_deref(), agent.as_deref())
        .await
    {
        Ok(()) => ok(),
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
    }
}

/// `POST /api/sessions/{id}/restore` — file restore; `confirm: true`
/// required. `{path, toolCallId?}` reverts recorded diffs;
/// `{checkpoint, paths?}` materializes a recorded snapshot ref.
pub async fn restore(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body.and_then(|b| b.as_object().cloned()) else {
        return bad_request("Expected a JSON object body");
    };
    if body.get("confirm") != Some(&Value::Bool(true)) {
        return bad_request("Restore requires confirm: true");
    }
    let path = match body.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("path must be a string"),
    };
    let tool_call_id = match body.get("toolCallId") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("toolCallId must be a string"),
    };
    let checkpoint = match body.get("checkpoint") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("checkpoint must be a string"),
    };
    let paths = match body.get("paths") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => Some(
            items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        ),
        _ => return bad_request("paths must be an array of strings"),
    };
    let request = RestoreRequest {
        confirm: true,
        path,
        tool_call_id,
        checkpoint,
        paths,
    };
    let result = state.plane.restore(&id, &request, agent.as_deref()).await;
    respond_map(result, StatusCode::OK, |r| json!(r))
}

/// `POST /api/sessions/{id}/rewind` — conversation rewind; exactly one
/// of `{nodeId, turns, checkpoint}`; `confirm: true` required.
pub async fn rewind(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let agent = agent_param(&req);
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body.and_then(|b| b.as_object().cloned()) else {
        return bad_request("Expected a JSON object body");
    };
    if body.get("confirm") != Some(&Value::Bool(true)) {
        return bad_request("Rewind requires confirm: true");
    }
    let int_field = |key: &str, min: f64| -> Result<Option<i64>, Box<Response>> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => match v.as_f64() {
                Some(n) if n.fract() == 0.0 && n >= min => Ok(Some(n as i64)),
                _ => Err(Box::new(bad_request(&format!(
                    "{key} must be a {} integer",
                    if min == 0.0 {
                        "non-negative"
                    } else {
                        "positive"
                    }
                )))),
            },
        }
    };
    let node_id = match int_field("nodeId", 0.0) {
        Ok(v) => v,
        Err(res) => return *res,
    };
    let turns = match int_field("turns", 1.0) {
        Ok(v) => v,
        Err(res) => return *res,
    };
    let checkpoint = match body.get("checkpoint") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("checkpoint must be a string"),
    };
    let request = RewindRequest {
        confirm: true,
        node_id,
        turns,
        checkpoint,
    };
    match state.plane.rewind(&id, &request, agent.as_deref()).await {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(result) => {
            state
                .feed
                .emit_session(&id, agent.as_deref(), json!({ "rewound": true }));
            json_response(json!(result), StatusCode::OK)
        }
    }
}

/* ---- convert / import -----------------------------------------------*/

/// `POST /api/sessions/{id}/convert` — convert in place between agent
/// stores (`{agent: "cline"|"devin"}`). 501 when no store backend is
/// wired.
pub async fn convert(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    let Some(convert) = &state.convert else {
        return not_configured("Convert");
    };
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let target = body
        .as_ref()
        .and_then(|b| b.as_object())
        .and_then(|o| o.get("agent"))
        .and_then(Value::as_str);
    let target = match target {
        Some("cline") => ImportTarget::Cline,
        Some("devin") => ImportTarget::Devin,
        _ => return bad_request("agent must be 'cline' or 'devin'"),
    };
    match convert(id.clone(), target).await {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(session_id) => {
            state.feed.emit_session(
                &session_id,
                Some(target.as_str()),
                json!({ "created": true }),
            );
            json_response(json!({ "sessionId": session_id }), StatusCode::OK)
        }
    }
}

/// `history` items ride `{role, content, createdAt, toolName?, …}` — the
/// flat compat form for sources too old to serve `{session}` IR.
const HISTORY_ROLES: [&str; 4] = ["system", "user", "assistant", "tool"];

fn parse_role(value: &str) -> Option<sepia_core::Role> {
    match value {
        "system" => Some(sepia_core::Role::System),
        "user" => Some(sepia_core::Role::User),
        "assistant" => Some(sepia_core::Role::Assistant),
        "tool" => Some(sepia_core::Role::Tool),
        _ => None,
    }
}

fn history_message(item: &Value) -> Result<sepia_convert::ImportedHistoryMessage, Box<Response>> {
    let Some(obj) = item.as_object() else {
        return Err(Box::new(bad_request(
            "history items must be { role, content, createdAt, toolName? } messages",
        )));
    };
    let role_ok = obj
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|r| HISTORY_ROLES.contains(&r));
    let content = obj.get("content").and_then(Value::as_str);
    let created_at = obj
        .get("createdAt")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite());
    let tool_name_ok = obj.get("toolName").is_none_or(Value::is_string);
    if !(role_ok && content.is_some() && created_at.is_some() && tool_name_ok) {
        return Err(Box::new(bad_request(
            "history items must be { role, content, createdAt, toolName? } messages",
        )));
    }
    let usage = obj
        .get("usage")
        .and_then(Value::as_object)
        .filter(|u| {
            u.get("input").and_then(Value::as_f64).is_some()
                && u.get("output").and_then(Value::as_f64).is_some()
        })
        .and_then(|u| serde_json::from_value(Value::Object(u.clone())).ok());
    let blocks = obj.get("blocks").and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter_map(|b| serde_json::from_value::<sepia_core::Block>(b.clone()).ok())
            .collect()
    });
    let tool_status = obj
        .get("toolStatus")
        .and_then(Value::as_str)
        .and_then(|s| match s {
            "pending" => Some(sepia_core::ToolCallStatus::Pending),
            "success" => Some(sepia_core::ToolCallStatus::Success),
            "error" => Some(sepia_core::ToolCallStatus::Error),
            _ => None,
        });
    Ok(sepia_convert::ImportedHistoryMessage {
        role: parse_role(obj["role"].as_str().unwrap_or_default())
            .unwrap_or(sepia_core::Role::User),
        content: content.unwrap_or_default().to_string(),
        blocks,
        created_at: created_at.unwrap_or_default(),
        tool_name: obj
            .get("toolName")
            .and_then(Value::as_str)
            .map(str::to_string),
        thinking: obj
            .get("thinking")
            .and_then(Value::as_str)
            .map(str::to_string),
        thinking_signature: obj
            .get("thinkingSignature")
            .and_then(Value::as_str)
            .map(str::to_string),
        usage,
        model: obj.get("model").and_then(Value::as_str).map(str::to_string),
        request_id: obj
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string),
        finish_reason: obj
            .get("finishReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_status,
        exit_code: obj
            .get("exitCode")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .map(|n| n as i64),
        duration_ms: obj
            .get("durationMs")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite()),
    })
}

/// `POST /api/sessions/import` — write explicit IR into an agent's
/// store. `{session}` carries the full IR verbatim (`GET .../export`);
/// `{history}` is the flat compat form.
pub async fn import_session(
    State(state): State<AppState>,
    method: Method,
    req: Request<Body>,
) -> Response {
    if method != Method::POST {
        return not_found();
    }
    // TS gates on `deps.convert` — conversion being configured is what
    // makes import possible; `importSession` is an override seam.
    if state.convert.is_none() {
        return not_configured("Import");
    }
    let body = match read_json_body(req).await {
        Ok(b) => b,
        Err(res) => return *res,
    };
    let Some(body) = body.and_then(|b| b.as_object().cloned()) else {
        return bad_request("Expected a JSON object body");
    };
    let target = match body.get("agent").and_then(Value::as_str) {
        Some("cline") => ImportTarget::Cline,
        Some("devin") => ImportTarget::Devin,
        _ => return bad_request("agent must be 'cline' or 'devin'"),
    };
    let cwd = match body.get("cwd") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => return bad_request("cwd must be a non-empty string"),
    };
    let title = match body.get("title") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("title must be a string"),
    };
    let model = match body.get("model") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return bad_request("model must be a string"),
    };

    let session: Session = if let Some(raw) = body.get("session") {
        // Full-IR form — decoded verbatim; a fresh id keeps import
        // semantics: every call lands as a new copy in the target store.
        let Ok(mut decoded) = sepia_core::wire::session_from_json(raw) else {
            return bad_request(
                "session must be a session IR object (GET /api/sessions/:id/export)",
            );
        };
        decoded.id = uuid::Uuid::new_v4().to_string();
        if let Some(t) = title.clone() {
            decoded.title = t;
        }
        if let Some(c) = cwd.clone() {
            decoded.working_directory = c;
        }
        if let Some(m) = model.clone() {
            decoded.model = m;
        }
        decoded
    } else {
        let Some(history) = body.get("history").and_then(Value::as_array) else {
            return bad_request("import requires a session IR object or a non-empty history array");
        };
        if history.is_empty() {
            return bad_request("import requires a session IR object or a non-empty history array");
        }
        let mut messages = Vec::with_capacity(history.len());
        for item in history {
            match history_message(item) {
                Ok(m) => messages.push(m),
                Err(res) => return *res,
            }
        }
        let first_user = messages.iter().find(|m| m.role == sepia_core::Role::User);
        let title = title.unwrap_or_else(|| {
            first_user.map_or_else(
                || "Imported session".to_string(),
                |m| m.content.chars().take(80).collect(),
            )
        });
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map_or_else(|_| "/".to_string(), |p| p.to_string_lossy().into_owned())
        });
        sepia_convert::session_from_history(
            &uuid::Uuid::new_v4().to_string(),
            &title,
            &cwd,
            model.as_deref().unwrap_or("sepia-import"),
            &messages,
        )
    };

    let run_span = RunSpan {
        at: now_ms_f64(),
        agent: target.as_str().to_string(),
        node: state.node.id.clone(),
    };
    let Some(import) = &state.import_session else {
        // Convert configured but no executor wired — an internal wiring
        // bug, same bucket the TS `mapError` puts executor failures in.
        return error_response(&ControlError::new(
            ControlErrorCode::Internal,
            "Import executor is not configured on this server",
        ));
    };
    match import(session.clone(), target).await {
        Err(e) => {
            tracing::warn!("request failed: {e}");
            error_response(&e)
        }
        Ok(session_id) => {
            state.feed.emit_session(
                &session_id,
                Some(target.as_str()),
                json!({ "created": true }),
            );
            // Provenance: the imported copy's run continues under
            // `target` on this node — same record an attach writes.
            if let Some(meta) = &state.meta {
                meta.add_span(&session_id, run_span.clone());
            }
            json_response(
                json!({
                    "id": session_id,
                    "title": session.title,
                    "cwd": session.working_directory,
                    "agent": target.as_str(),
                    "updatedAt": iso_of_secs(session.last_activity_at),
                    "locked": false,
                    "lockHolderPid": null,
                    "source": target.as_str(),
                    "busy": false,
                    "spans": if state.meta.is_some() { json!([run_span]) } else { json!([]) },
                }),
                StatusCode::CREATED,
            )
        }
    }
}
