//! `SepiaEntityPlane` — the server-side `kind="sepia-session"`
//! [`hypha_http::EntityPlane`] impl. Where [`crate::SepiaPeregrin`] is
//! the *client* (it speaks `/api/entities*` at a node), this plane is
//! what a hypha node host registers via
//! [`hypha_http::HyphaState::with_kind`] so the substrate's entity
//! surface serves sepia sessions natively: entity docs project
//! [`sepia_session::SessionSummary`] rows, `sepia.<verb>` ops dispatch
//! onto [`sepia_session::ControlPlane`] (through [`Local`]'s wider
//! surface — import/convert/store-delete), and snapshots are the
//! session IR export/import pair.
//!
//! Unlike peregrin's plane this one never downcasts `EntityContext::host`
//! — the backend is taken at construction, so the same plane serves
//! whatever session plane the host composed. The `?agent=` store
//! disambiguator is this kind's own query param, threaded into the
//! session calls exactly the way `sepia serve` threads its `agent`
//! param; it never leaks into substrate dispatch.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hypha_http::{EntityContext, EntityPlane};
use serde_json::{Value, json};

use sepia_acp::PromptPart;
use sepia_core::Session;
use sepia_meta::SessionMeta;
use sepia_session::{
    AttachResult, ControlError, ControlErrorCode, CreateResult, ImportOverrides, Local,
    RestoreRequest, RestoreResult, RewindRequest, RewindResult, SessionSummary, agent_for_backend,
};

use crate::doc::ENTITY_KIND;
use crate::doc::summary_to_entity;

/// The session-plane surface the entity plane needs — everything
/// `sepia serve` exposes minus the transport. [`Local`] is the
/// production impl; tests substitute a fake.
#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    /// Store summaries + live/pending rows; `with_locks` probes agent
    /// lock state (the `?withLocks=1` list param).
    async fn list_sessions(&self, with_locks: bool) -> Result<Vec<SessionSummary>, ControlError>;

    /// One session's summary row — meta overlay applied — for
    /// `GET /api/entities/{id}`.
    async fn summary(&self, id: &str, agent: Option<&str>) -> Result<SessionSummary, ControlError>;

    /// The complete session IR — the snapshot/export source.
    async fn session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError>;

    /// `POST /api/entities` — spawn an agent, mint a fresh session.
    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError>;

    /// Live-attach an existing session (`takeover`, spawn `model` +
    /// `fallbacks` overrides).
    async fn attach(
        &self,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent: Option<String>,
    ) -> Result<AttachResult, ControlError>;

    /// Release a live attach — a no-op when not attached.
    async fn detach(&self, id: &str);

    /// One prompt turn — the ACP `session/prompt` content-block list.
    async fn prompt(
        &self,
        id: &str,
        parts: &[PromptPart],
        agent: Option<&str>,
    ) -> Result<(), ControlError>;

    /// Fire-and-forget turn cancellation.
    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError>;

    /// Settle a pending permission request.
    async fn permission(
        &self,
        id: &str,
        request_id: &str,
        option_id: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), ControlError>;

    /// File-level restore — `RestoreRequest::confirm` is mandatory.
    async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent: Option<&str>,
    ) -> Result<RestoreResult, ControlError>;

    /// Conversation rewind — `RewindRequest::confirm` is mandatory.
    async fn rewind(
        &self,
        id: &str,
        request: &RewindRequest,
        agent: Option<&str>,
    ) -> Result<RewindResult, ControlError>;

    /// Delete through the agent runtime (`session/delete`); callers
    /// fall back to [`Self::delete_session_store`] on `unknown_agent`.
    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError>;

    /// Store-level delete over the driver wire — the fallback for
    /// agents with no runtime (cursor).
    async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError>;

    /// The meta-overlay patch — title/pin/archive/projects/model.
    async fn patch_meta(&self, id: &str, patch: &SessionMeta) -> Result<(), ControlError>;

    /// Write session IR into `agent`'s store — a fresh id is minted
    /// unless `overrides.id` re-keys it.
    async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError>;

    /// Node-local store→store copy: read `id` (from `from`'s store when
    /// ids collide) and import it into `agent`'s.
    async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError>;
}

/// Merge the meta overlay into a summary row — the same projection
/// `ControlPlane::to_summary` applies, for the single-get path where
/// the store row (not `list_sessions`) is the source.
fn apply_meta_overlay(summary: &mut SessionSummary, meta: Option<SessionMeta>) {
    let Some(m) = meta else { return };
    if let Some(title) = m.title {
        summary.title = title;
    }
    summary.pinned = m.pinned;
    summary.archived = m.archived;
    summary.project_ids = m.project_ids;
    summary.spans = m
        .spans
        .map(|spans| spans.into_iter().map(Into::into).collect());
}

/// Epoch seconds → RFC 3339 — mirrors `ControlPlane`'s `iso_of`.
fn iso_of(epoch_secs: f64) -> String {
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp_nanos((epoch_secs * 1e9) as i128) else {
        return String::new();
    };
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl SessionBackend for Local {
    async fn list_sessions(&self, with_locks: bool) -> Result<Vec<SessionSummary>, ControlError> {
        self.plane.list_sessions(with_locks).await
    }

    async fn summary(&self, id: &str, agent: Option<&str>) -> Result<SessionSummary, ControlError> {
        // The `GET /api/sessions/{id}` projection: store row → summary
        // fields the list path derives, then the meta overlay on top.
        let session = self.plane.get_summary(id, agent).await?;
        let can_rewind = self.plane.can_rewind(&session.backend_type);
        let mut summary = SessionSummary {
            id: session.id.clone(),
            title: session.title.clone(),
            cwd: session.working_directory.clone(),
            agent: agent_for_backend(&session.backend_type).to_string(),
            updated_at: iso_of(session.last_activity_at),
            locked: false,
            lock_holder_pid: None,
            source: agent_for_backend(&session.backend_type).to_string(),
            busy: false,
            pinned: None,
            archived: None,
            project_ids: None,
            spans: None,
            parent_session_id: session.parent_session_id.clone(),
            agent_id: session.agent_id.clone(),
            can_rewind,
        };
        apply_meta_overlay(&mut summary, self.meta.of(id));
        Ok(summary)
    }

    async fn session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError> {
        self.plane.get_session(id, agent).await
    }

    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError> {
        self.plane
            .create_session(cwd, agent, title, model, fallbacks)
            .await
    }

    async fn attach(
        &self,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent: Option<String>,
    ) -> Result<AttachResult, ControlError> {
        self.plane
            .attach(id, takeover, model, fallbacks, agent)
            .await
    }

    async fn detach(&self, id: &str) {
        self.plane.detach(id).await;
    }

    async fn prompt(
        &self,
        id: &str,
        parts: &[PromptPart],
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.plane.prompt(id, parts, agent).await
    }

    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        self.plane.cancel(id, agent).await
    }

    async fn permission(
        &self,
        id: &str,
        request_id: &str,
        option_id: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.plane
            .respond_to_permission(id, request_id, option_id, agent)
            .await
    }

    async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent: Option<&str>,
    ) -> Result<RestoreResult, ControlError> {
        self.plane.restore(id, request, agent).await
    }

    async fn rewind(
        &self,
        id: &str,
        request: &RewindRequest,
        agent: Option<&str>,
    ) -> Result<RewindResult, ControlError> {
        self.plane.rewind(id, request, agent).await
    }

    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        self.plane.delete_session(id, agent).await
    }

    async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError> {
        Local::delete_session_store(self, agent, id).await
    }

    async fn patch_meta(&self, id: &str, patch: &SessionMeta) -> Result<(), ControlError> {
        self.plane.patch_meta(id, patch).await
    }

    async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError> {
        Local::import_session(self, agent, session, overrides).await
    }

    async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError> {
        Local::convert_session(self, id, agent, from).await
    }
}

/* ---- wire helpers ----------------------------------------------------*/

fn json_response(body: Value, status: StatusCode) -> Response {
    (status, axum::Json(body)).into_response()
}

/// The `ControlError` → status mapping `sepia serve`/`peregrin-http`
/// share: not_found→404, invalid/unknown_agent→400,
/// locked/conflict/busy→409, internal→500.
fn code_status(code: ControlErrorCode) -> StatusCode {
    match code {
        ControlErrorCode::NotFound => StatusCode::NOT_FOUND,
        ControlErrorCode::Invalid | ControlErrorCode::UnknownAgent => StatusCode::BAD_REQUEST,
        ControlErrorCode::Locked | ControlErrorCode::Conflict | ControlErrorCode::Busy => {
            StatusCode::CONFLICT
        }
        ControlErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn error_response(error: &ControlError) -> Response {
    json_response(
        json!({ "error": error.message, "code": error.code.as_str() }),
        code_status(error.code),
    )
}

fn ok() -> Response {
    json_response(json!({ "ok": true }), StatusCode::OK)
}

fn bad_request(message: &str) -> Response {
    json_response(json!({ "error": message }), StatusCode::BAD_REQUEST)
}

/// `?key=` out of the raw query string — the substrate hands planes the
/// undecoded `?…` tail; each kind parses its own params.
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    })
}

/// The `?agent=` store disambiguator — empty reads as absent.
fn agent_of(query: Option<&str>) -> Option<String> {
    query_param(query, "agent").filter(|a| !a.is_empty())
}

fn str_field<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

fn opt_str(body: &Value, key: &str) -> Option<String> {
    str_field(body, key).map(str::to_string)
}

fn opt_str_list(body: &Value, key: &str) -> Option<Vec<String>> {
    body.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect()
    })
}

/// `{text?, attachments?}` or `{parts: [...]}` → the ACP content-block
/// list — the same two shapes `POST /api/sessions/{id}/prompt` accepts.
fn prompt_parts(payload: &Value) -> Result<Vec<PromptPart>, String> {
    if let Some(parts) = payload.get("parts") {
        let items = parts
            .as_array()
            .ok_or_else(|| "parts must be an array of content blocks".to_string())?;
        return items
            .iter()
            .map(|p| {
                serde_json::from_value::<PromptPart>(p.clone())
                    .map_err(|_| "parts must be ACP content blocks".to_string())
            })
            .collect();
    }
    let mut parts: Vec<PromptPart> = Vec::new();
    if let Some(text) = str_field(payload, "text").filter(|t| !t.trim().is_empty()) {
        parts.push(PromptPart::Text {
            text: text.to_string(),
        });
    }
    if let Some(attachments) = payload.get("attachments") {
        let items = attachments
            .as_array()
            .ok_or_else(|| "attachments must be an array of content blocks".to_string())?;
        for item in items {
            parts.push(
                serde_json::from_value::<PromptPart>(item.clone())
                    .map_err(|_| "attachments must be ACP content blocks".to_string())?,
            );
        }
    }
    if parts.is_empty() {
        return Err("text, parts, or attachments is required".to_string());
    }
    Ok(parts)
}

/// The meta-patch payload — `{patch: {...}}` (the `sessions.patch_meta`
/// wire shape) or the flat item-PATCH fields (`title`, `pinned`,
/// `archived`, `projectIds`, `model`). `model: null` is the documented
/// explicit clear (`Option<Option<String>>` flattens it otherwise).
fn meta_patch(payload: &Value) -> Result<SessionMeta, String> {
    let raw = payload
        .get("patch")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    let mut patch: SessionMeta = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
    if raw.get("model").is_some() {
        patch.model = match raw["model"].as_str() {
            Some(s) => Some(Some(s.to_string())),
            None => Some(None),
        };
    }
    Ok(patch)
}

/// A flat `{role, content, createdAt, …}` history row →
/// [`sepia_convert::ImportedHistoryMessage`] — the compat import form.
fn history_message(item: &Value) -> Result<sepia_convert::ImportedHistoryMessage, String> {
    let role = match str_field(item, "role") {
        Some("system") => sepia_core::Role::System,
        Some("user") => sepia_core::Role::User,
        Some("assistant") => sepia_core::Role::Assistant,
        Some("tool") => sepia_core::Role::Tool,
        _ => return Err("history role must be system|user|assistant|tool".to_string()),
    };
    let content = str_field(item, "content")
        .ok_or("history items need a content string")?
        .to_string();
    let created_at = item
        .get("createdAt")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or("history items need a finite createdAt")?;
    let blocks = item.get("blocks").and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter_map(|b| serde_json::from_value::<sepia_core::Block>(b.clone()).ok())
            .collect()
    });
    let tool_status = str_field(item, "toolStatus").and_then(|s| match s {
        "pending" => Some(sepia_core::ToolCallStatus::Pending),
        "success" => Some(sepia_core::ToolCallStatus::Success),
        "error" => Some(sepia_core::ToolCallStatus::Error),
        _ => None,
    });
    Ok(sepia_convert::ImportedHistoryMessage {
        role,
        content,
        blocks,
        created_at,
        tool_name: opt_str(item, "toolName"),
        thinking: opt_str(item, "thinking"),
        thinking_signature: opt_str(item, "thinkingSignature"),
        usage: item
            .get("usage")
            .and_then(|u| serde_json::from_value(u.clone()).ok()),
        model: opt_str(item, "model"),
        request_id: opt_str(item, "requestId"),
        finish_reason: opt_str(item, "finishReason"),
        tool_status,
        exit_code: item.get("exitCode").and_then(Value::as_i64),
        duration_ms: item.get("durationMs").and_then(Value::as_f64),
    })
}

/// The `sepia-session` entity plane — see the module docs. Construct
/// with [`Self::new`] over any [`SessionBackend`]; `Arc<Local>` is the
/// production wiring a node host passes to `HyphaState::with_kind`.
pub struct SepiaEntityPlane {
    backend: Arc<dyn SessionBackend>,
}

impl SepiaEntityPlane {
    /// A plane over `backend` — typically `Arc<Local>` built by
    /// `sepia_session::build`.
    #[must_use]
    pub fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self { backend }
    }

    /// Convenience for the common case — `Local` as the backend.
    #[must_use]
    pub fn local(local: Arc<Local>) -> Self {
        Self::new(local)
    }

    /// Dispatch one `sepia.<verb>` op — returns `None` for verbs this
    /// kind doesn't own (the substrate's 501 path handles those).
    async fn dispatch_op(
        &self,
        id: &str,
        verb: &str,
        payload: &Value,
        agent: Option<&str>,
    ) -> Option<Response> {
        let backend = &self.backend;
        match verb {
            "attach" => {
                let takeover = payload.get("takeover") == Some(&Value::Bool(true));
                let result = backend
                    .attach(
                        id,
                        takeover,
                        opt_str(payload, "model"),
                        opt_str_list(payload, "fallbacks"),
                        agent.map(str::to_string),
                    )
                    .await;
                Some(match result {
                    Ok(r) => json_response(
                        json!({
                            "attached": r.attached,
                            "readOnly": r.read_only,
                            "agentId": r.agent_id,
                            "capabilities": r.capabilities,
                        }),
                        StatusCode::OK,
                    ),
                    Err(e) => error_response(&e),
                })
            }
            "detach" => {
                backend.detach(id).await;
                Some(ok())
            }
            "prompt" => match prompt_parts(payload) {
                Ok(parts) => Some(match backend.prompt(id, &parts, agent).await {
                    Ok(()) => ok(),
                    Err(e) => error_response(&e),
                }),
                Err(message) => Some(bad_request(&message)),
            },
            "cancel" => Some(match backend.cancel(id, agent).await {
                Ok(()) => ok(),
                Err(e) => error_response(&e),
            }),
            "permission" => {
                let Some(request_id) = str_field(payload, "requestId") else {
                    return Some(bad_request("requestId is required"));
                };
                let option_id = match payload.get("optionId") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => return Some(bad_request("optionId must be a string or null")),
                };
                Some(
                    match backend
                        .permission(id, request_id, option_id.as_deref(), agent)
                        .await
                    {
                        Ok(()) => ok(),
                        Err(e) => error_response(&e),
                    },
                )
            }
            "restore" | "rewind" => {
                if payload.get("confirm") != Some(&Value::Bool(true)) {
                    let what = if verb == "restore" {
                        "Restore"
                    } else {
                        "Rewind"
                    };
                    return Some(bad_request(&format!("{what} requires confirm: true")));
                }
                if verb == "restore" {
                    match serde_json::from_value::<RestoreRequest>(payload.clone()) {
                        Ok(request) => Some(match backend.restore(id, &request, agent).await {
                            Ok(r) => json_response(json!(r), StatusCode::OK),
                            Err(e) => error_response(&e),
                        }),
                        Err(e) => Some(bad_request(&e.to_string())),
                    }
                } else {
                    match serde_json::from_value::<RewindRequest>(payload.clone()) {
                        Ok(request) => Some(match backend.rewind(id, &request, agent).await {
                            Ok(r) => json_response(json!(r), StatusCode::OK),
                            Err(e) => error_response(&e),
                        }),
                        Err(e) => Some(bad_request(&e.to_string())),
                    }
                }
            }
            "convert" => {
                // `{agent|to: "<target>", from?: "<source>"}` — the target
                // store is explicit in the payload; `?agent=` stays the
                // session's source disambiguator.
                let target = opt_str(payload, "agent")
                    .or_else(|| opt_str(payload, "to"))
                    .or_else(|| agent.map(str::to_string));
                let Some(target) = target else {
                    return Some(bad_request("convert needs an agent (target store)"));
                };
                let from = opt_str(payload, "from").or_else(|| agent.map(str::to_string));
                Some(
                    match backend.convert_session(id, &target, from.as_deref()).await {
                        Ok(session_id) => json_response(
                            json!({ "sessionId": session_id, "agentId": target }),
                            StatusCode::OK,
                        ),
                        Err(e) => error_response(&e),
                    },
                )
            }
            "meta" | "meta.patch" => match meta_patch(payload) {
                Ok(patch) => {
                    if patch == SessionMeta::default() {
                        return Some(bad_request("Nothing to patch"));
                    }
                    Some(match backend.patch_meta(id, &patch).await {
                        Ok(()) => ok(),
                        Err(e) => error_response(&e),
                    })
                }
                Err(e) => Some(bad_request(&e)),
            },
            "delete" | "entity.delete" => {
                // Same path as `DELETE /api/entities/{id}` — the plane
                // owns it once `get` claimed the id.
                Some(SepiaEntityPlane::delete(self, id, agent.map(str::to_string)).await)
            }
            _ => None,
        }
    }

    async fn delete(&self, id: &str, agent: Option<String>) -> Response {
        match self.backend.delete_session(id, agent.as_deref()).await {
            // A created-but-unflushed session isn't in the repo —
            // deleting it is still a success (the route's long-standing
            // semantic).
            Err(e) if e.code == ControlErrorCode::NotFound => ok(),
            // Agents without a runtime (cursor) can't serve ACP
            // `session/delete` — fall back to the store-level delete
            // over the driver wire when the request names the agent,
            // same as `sepia serve`.
            Err(e) if e.code == ControlErrorCode::UnknownAgent && agent.is_some() => {
                match self
                    .backend
                    .delete_session_store(agent.as_deref().unwrap_or_default(), id)
                    .await
                {
                    Ok(()) => ok(),
                    Err(e) => error_response(&e),
                }
            }
            Err(e) => error_response(&e),
            Ok(()) => ok(),
        }
    }
}

#[async_trait::async_trait]
impl EntityPlane for SepiaEntityPlane {
    fn kind(&self) -> &'static str {
        ENTITY_KIND
    }

    /// `sepia.*` is the op namespace — `sepia.prompt`, `sepia.meta.patch`, …
    fn op_namespace(&self) -> &'static str {
        "sepia"
    }

    /// `meta.*` mirrors the reference plane: bare `meta.patch` ops
    /// (no `sepia.` prefix) still reach this kind's table.
    fn extra_op_namespaces(&self) -> &'static [&'static str] {
        &["meta"]
    }

    async fn list(&self, _ctx: &EntityContext, query: Option<&str>) -> Vec<Value> {
        let with_locks = query_param(query, "withLocks").as_deref() == Some("1");
        match self.backend.list_sessions(with_locks).await {
            Ok(rows) => rows
                .iter()
                .filter_map(|s| serde_json::to_value(s).ok())
                .map(|v| summary_to_entity(&v))
                .collect(),
            // Same silent-200 the list route always had.
            Err(_) => Vec::new(),
        }
    }

    async fn get(&self, _ctx: &EntityContext, id: &str, query: Option<&str>) -> Response {
        let agent = agent_of(query);
        match self.backend.summary(id, agent.as_deref()).await {
            Ok(summary) => match serde_json::to_value(&summary) {
                Ok(v) => json_response(summary_to_entity(&v), StatusCode::OK),
                Err(e) => json_response(
                    json!({ "error": e.to_string() }),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            },
            Err(e) => error_response(&e),
        }
    }

    async fn create(&self, _ctx: &EntityContext, init: Value) -> Response {
        // `init` is the body's `indexDoc` (or the whole body) — the
        // session-create shape: `cwd` required, `agent`/`title`/`model`/
        // `fallbacks` optional.
        let Some(cwd) = str_field(&init, "cwd") else {
            return bad_request("cwd is required");
        };
        if cwd.trim().is_empty() {
            return bad_request("cwd is required");
        }
        match self
            .backend
            .create_session(
                cwd,
                opt_str(&init, "agent"),
                opt_str(&init, "title"),
                opt_str(&init, "model"),
                opt_str_list(&init, "fallbacks"),
            )
            .await
        {
            Ok(created) => json_response(
                json!({
                    "id": created.id,
                    "agentId": created.agent_id,
                    "capabilities": created.capabilities,
                }),
                StatusCode::CREATED,
            ),
            Err(e) => error_response(&e),
        }
    }

    async fn delete(&self, _ctx: &EntityContext, id: &str, query: Option<&str>) -> Response {
        SepiaEntityPlane::delete(self, id, agent_of(query)).await
    }

    async fn op(
        &self,
        _ctx: &EntityContext,
        id: &str,
        op: &str,
        payload: Value,
        query: Option<&str>,
    ) -> Response {
        // `sepia.` strips; un-namespaced ops (legacy outbox rows) and
        // `meta.*`-namespaced ops (`meta.patch` arrives verbatim) read
        // as session-plane verbs.
        let verb = op.strip_prefix("sepia.").unwrap_or(op);
        let agent = agent_of(query);
        match self.dispatch_op(id, verb, &payload, agent.as_deref()).await {
            Some(response) => response,
            None => json_response(
                json!({ "error": format!("unknown op {op:?}") }),
                StatusCode::NOT_IMPLEMENTED,
            ),
        }
    }

    async fn export(&self, _ctx: &EntityContext, id: &str, query: Option<&str>) -> Response {
        let agent = agent_of(query);
        match self.backend.session(id, agent.as_deref()).await {
            Ok(session) => match sepia_core::wire::session_to_json(&session) {
                Ok(session) => json_response(json!({ "session": session }), StatusCode::OK),
                Err(e) => json_response(
                    json!({ "error": e.to_string() }),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            },
            Err(e) => error_response(&e),
        }
    }

    async fn import(&self, _ctx: &EntityContext, body: Value) -> Response {
        // `{session: <IR>, agent, id?, cwd?, title?, model?}` — the
        // `sessions.import` wire shape verbatim; `{history: [...]}` is
        // the flat compat form rebuilt through `sepia-convert`.
        let Some(_obj) = body.as_object() else {
            return bad_request("Expected a JSON object body");
        };
        let Some(agent) = str_field(&body, "agent").map(str::to_string) else {
            return bad_request("agent is required");
        };
        let overrides = ImportOverrides {
            id: opt_str(&body, "id").filter(|id| !id.is_empty()),
            cwd: opt_str(&body, "cwd"),
            title: opt_str(&body, "title"),
            model: opt_str(&body, "model"),
        };
        let session = if let Some(raw) = body.get("session") {
            match sepia_core::wire::session_from_json(raw) {
                Ok(session) => session,
                Err(_) => {
                    return bad_request(
                        "session must be a session IR object (GET /api/entities/{id}/snapshot)",
                    );
                }
            }
        } else {
            let Some(history) = body.get("history").and_then(Value::as_array) else {
                return bad_request(
                    "import requires a session IR object or a non-empty history array",
                );
            };
            if history.is_empty() {
                return bad_request(
                    "import requires a session IR object or a non-empty history array",
                );
            }
            let mut messages = Vec::with_capacity(history.len());
            for item in history {
                match history_message(item) {
                    Ok(m) => messages.push(m),
                    Err(e) => return bad_request(&e),
                }
            }
            let first_user = messages.iter().find(|m| m.role == sepia_core::Role::User);
            let title = overrides.title.clone().unwrap_or_else(|| {
                first_user.map_or_else(
                    || "Imported session".to_string(),
                    |m| m.content.chars().take(80).collect(),
                )
            });
            let cwd = overrides.cwd.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map_or_else(|_| "/".to_string(), |p| p.to_string_lossy().into_owned())
            });
            sepia_convert::session_from_history(
                &uuid::Uuid::new_v4().to_string(),
                &title,
                &cwd,
                overrides.model.as_deref().unwrap_or("sepia-import"),
                &messages,
            )
        };
        match self
            .backend
            .import_session(&agent, &session, &overrides)
            .await
        {
            Ok(id) => json_response(json!({ "id": id, "agentId": agent }), StatusCode::OK),
            Err(e) => error_response(&e),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::Mutex;

    use hypha_http::KindRegistry;
    use sepia_acp::PromptPart;
    use sepia_core::Session;
    use sepia_meta::SessionMeta;
    use sepia_session::{
        AttachResult, ControlError, ControlErrorCode, CreateResult, ImportOverrides,
        RestoreRequest, RestoreResult, RewindRequest, RewindResult, SessionSummary,
    };

    /// Records every call (`"verb:id:agent"`) and serves canned data.
    struct Fake {
        calls: Mutex<Vec<String>>,
        summaries: Vec<SessionSummary>,
        session: Option<Session>,
        /// When set, every fallible verb fails with this error.
        error: Option<ControlError>,
        /// `delete_session` fails `unknown_agent` while
        /// `delete_session_store` succeeds — the cursor fallback path.
        delete_unknown_agent: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                summaries: Vec::new(),
                session: None,
                error: None,
                delete_unknown_agent: false,
            }
        }

        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn fail<T>(&self) -> Result<T, ControlError> {
            Err(self
                .error
                .clone()
                .unwrap_or_else(|| ControlError::new(ControlErrorCode::Internal, "fake failure")))
        }
    }

    #[async_trait::async_trait]
    impl SessionBackend for Fake {
        async fn list_sessions(
            &self,
            with_locks: bool,
        ) -> Result<Vec<SessionSummary>, ControlError> {
            self.record(format!("list:{with_locks}"));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(self.summaries.clone())
        }

        async fn summary(
            &self,
            id: &str,
            agent: Option<&str>,
        ) -> Result<SessionSummary, ControlError> {
            self.record(format!("summary:{id}:{}", agent.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(self
                .summaries
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .unwrap_or(SessionSummary {
                    id: id.to_string(),
                    title: "T".into(),
                    cwd: "/w".into(),
                    agent: "devin".into(),
                    updated_at: "2024-01-01T00:00:00Z".into(),
                    locked: false,
                    lock_holder_pid: None,
                    source: "devin".into(),
                    busy: false,
                    pinned: None,
                    archived: None,
                    project_ids: None,
                    spans: None,
                    parent_session_id: None,
                    agent_id: None,
                    can_rewind: false,
                }))
        }

        async fn session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError> {
            self.record(format!("session:{id}:{}", agent.unwrap_or("-")));
            self.session.clone().ok_or_else(|| {
                ControlError::new(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })
        }

        async fn create_session(
            &self,
            cwd: &str,
            agent: Option<String>,
            title: Option<String>,
            model: Option<String>,
            fallbacks: Option<Vec<String>>,
        ) -> Result<CreateResult, ControlError> {
            self.record(format!(
                "create:{cwd}:{}:{title:?}:{model:?}:{fallbacks:?}",
                agent.unwrap_or_default()
            ));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(CreateResult {
                id: "new-1".into(),
                agent_id: "devin".into(),
                capabilities: sepia_acp::AcpCapabilities::default(),
            })
        }

        async fn attach(
            &self,
            id: &str,
            takeover: bool,
            model: Option<String>,
            fallbacks: Option<Vec<String>>,
            agent: Option<String>,
        ) -> Result<AttachResult, ControlError> {
            self.record(format!(
                "attach:{id}:{takeover}:{model:?}:{fallbacks:?}:{}",
                agent.unwrap_or_default()
            ));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(AttachResult {
                attached: true,
                read_only: false,
                agent_id: "devin".into(),
                capabilities: sepia_acp::AcpCapabilities::default(),
            })
        }

        async fn detach(&self, id: &str) {
            self.record(format!("detach:{id}"));
        }

        async fn prompt(
            &self,
            id: &str,
            parts: &[PromptPart],
            agent: Option<&str>,
        ) -> Result<(), ControlError> {
            self.record(format!("prompt:{id}:{}:{parts:?}", agent.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(())
        }

        async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
            self.record(format!("cancel:{id}:{}", agent.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(())
        }

        async fn permission(
            &self,
            id: &str,
            request_id: &str,
            option_id: Option<&str>,
            agent: Option<&str>,
        ) -> Result<(), ControlError> {
            self.record(format!(
                "permission:{id}:{request_id}:{option_id:?}:{}",
                agent.unwrap_or("-")
            ));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(())
        }

        async fn restore(
            &self,
            id: &str,
            request: &RestoreRequest,
            agent: Option<&str>,
        ) -> Result<RestoreResult, ControlError> {
            self.record(format!("restore:{id}:{request:?}:{}", agent.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(RestoreResult {
                restored: vec![],
                skipped: vec![],
            })
        }

        async fn rewind(
            &self,
            id: &str,
            request: &RewindRequest,
            agent: Option<&str>,
        ) -> Result<RewindResult, ControlError> {
            self.record(format!("rewind:{id}:{request:?}:{}", agent.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(RewindResult {
                kept: 1,
                removed: 2,
            })
        }

        async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
            self.record(format!("delete:{id}:{}", agent.unwrap_or("-")));
            if self.delete_unknown_agent {
                return Err(ControlError::new(
                    ControlErrorCode::UnknownAgent,
                    "no runtime",
                ));
            }
            if self.error.is_some() {
                return self.fail();
            }
            Ok(())
        }

        async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError> {
            self.record(format!("delete_store:{agent}:{id}"));
            Ok(())
        }

        async fn patch_meta(&self, id: &str, patch: &SessionMeta) -> Result<(), ControlError> {
            self.record(format!("patch_meta:{id}:{patch:?}"));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(())
        }

        async fn import_session(
            &self,
            agent: &str,
            session: &Session,
            overrides: &ImportOverrides,
        ) -> Result<String, ControlError> {
            self.record(format!("import:{agent}:{}:{overrides:?}", session.id));
            if self.error.is_some() {
                return self.fail();
            }
            Ok(overrides.id.clone().unwrap_or_else(|| "imported-1".into()))
        }

        async fn convert_session(
            &self,
            id: &str,
            agent: &str,
            from: Option<&str>,
        ) -> Result<String, ControlError> {
            self.record(format!("convert:{id}:{agent}:{}", from.unwrap_or("-")));
            if self.error.is_some() {
                return self.fail();
            }
            Ok("converted-1".into())
        }
    }

    fn plane(fake: Fake) -> SepiaEntityPlane {
        SepiaEntityPlane::new(Arc::new(fake))
    }

    fn ctx() -> EntityContext {
        EntityContext::for_kinds(KindRegistry::new())
    }

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: "T".into(),
            cwd: "/w".into(),
            agent: "devin".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
            locked: true,
            lock_holder_pid: Some(42.0),
            source: "devin".into(),
            busy: true,
            pinned: Some(true),
            archived: None,
            project_ids: Some(vec!["p1".into()]),
            spans: None,
            parent_session_id: None,
            agent_id: None,
            can_rewind: true,
        }
    }

    async fn body_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn kind_and_namespaces() {
        let p = plane(Fake::new());
        assert_eq!(p.kind(), "sepia-session");
        assert_eq!(p.op_namespace(), "sepia");
        assert_eq!(p.extra_op_namespaces(), &["meta"]);
    }

    #[tokio::test]
    async fn list_projects_summaries_to_entity_docs() {
        let mut fake = Fake::new();
        fake.summaries = vec![summary("s1")];
        let p = plane(fake);
        let docs = p.list(&ctx(), Some("withLocks=1")).await;
        assert_eq!(docs.len(), 1);
        let doc = &docs[0];
        assert_eq!(doc["id"], "s1");
        assert_eq!(doc["kind"], "sepia-session");
        assert_eq!(doc["locked"], true);
        assert_eq!(doc["live"], true); // busy lifts to live
        assert_eq!(doc["indexDoc"]["title"], "T");
    }

    #[tokio::test]
    async fn list_degrades_to_empty_on_backend_error() {
        let mut fake = Fake::new();
        fake.error = Some(ControlError::new(ControlErrorCode::Internal, "boom"));
        let p = plane(fake);
        assert_eq!(p.list(&ctx(), None).await, Vec::<Value>::new());
    }

    #[tokio::test]
    async fn get_returns_entity_doc_and_threads_agent() {
        let fake = Fake::new();
        // Recover the fake's call log through the plane? The plane owns
        // the Arc — keep a second handle by wrapping manually instead.
        let fake = Arc::new(fake);
        let p = SepiaEntityPlane::new(fake.clone());
        let response = p.get(&ctx(), "s1", Some("agent=devin&x=1")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let doc = body_json(response).await;
        assert_eq!(doc["id"], "s1");
        assert_eq!(doc["kind"], "sepia-session");
        assert_eq!(fake.calls(), vec!["summary:s1:devin"]);
    }

    #[tokio::test]
    async fn get_maps_not_found_to_404() {
        let mut fake = Fake::new();
        fake.error = Some(ControlError::new(ControlErrorCode::NotFound, "gone"));
        let p = plane(fake);
        let response = p.get(&ctx(), "s1", None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert_eq!(body["code"], "not_found");
    }

    #[tokio::test]
    async fn create_validates_and_returns_201() {
        let fake = Fake::new();
        let fake = Arc::new(fake);
        let p = SepiaEntityPlane::new(fake.clone());
        // Missing cwd → 400.
        let response = p.create(&ctx(), json!({})).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // Full init → 201 with the create payload.
        let response = p
            .create(
                &ctx(),
                json!({"cwd": "/work", "agent": "devin", "title": "T", "model": "m", "fallbacks": ["f"]}),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(body["id"], "new-1");
        assert_eq!(body["agentId"], "devin");
        let calls = fake.calls();
        assert!(calls[0].contains("create:/work:devin"), "{calls:?}");
    }

    #[tokio::test]
    async fn op_prompt_translates_text_and_threads_agent() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let response = p
            .op(
                &ctx(),
                "s1",
                "sepia.prompt",
                json!({"text": "hi"}),
                Some("agent=cline"),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("prompt:s1:cline:"), "{calls:?}");
        assert!(calls[0].contains("Text"), "{calls:?}");
    }

    #[tokio::test]
    async fn op_prompt_accepts_parts_array() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let response = p
            .op(
                &ctx(),
                "s1",
                "prompt", // bare verb — legacy un-namespaced row
                json!({"parts": [{"type": "text", "text": "yo"}]}),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(fake.calls()[0].contains("prompt:s1:-:"));
    }

    #[tokio::test]
    async fn op_prompt_rejects_empty_payload() {
        let p = plane(Fake::new());
        let response = p.op(&ctx(), "s1", "sepia.prompt", json!({}), None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn op_permission_and_cancel() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p
            .op(
                &ctx(),
                "s1",
                "sepia.permission",
                json!({"requestId": "r1", "optionId": "allow"}),
                None,
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let r = p.op(&ctx(), "s1", "sepia.cancel", json!({}), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            fake.calls(),
            vec!["permission:s1:r1:Some(\"allow\"):-", "cancel:s1:-"]
        );
    }

    #[tokio::test]
    async fn op_restore_and_rewind_require_confirm() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p
            .op(&ctx(), "s1", "sepia.restore", json!({"path": "a"}), None)
            .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = p
            .op(
                &ctx(),
                "s1",
                "sepia.rewind",
                json!({"confirm": true, "turns": 2}),
                Some("agent=devin"),
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_json(r).await;
        assert_eq!(body["kept"], 1);
        assert!(fake.calls()[0].contains("rewind:s1:"), "{:?}", fake.calls());
        assert!(fake.calls()[0].contains("devin"));
    }

    #[tokio::test]
    async fn op_meta_patch_flat_and_wrapped() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        // Flat item-PATCH shape.
        let r = p
            .op(
                &ctx(),
                "s1",
                "sepia.meta.patch",
                json!({"title": "New", "pinned": true}),
                None,
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        // `sessions.patch_meta` wire shape — `{patch: {...}}`.
        let r = p
            .op(
                &ctx(),
                "s1",
                "meta.patch",
                json!({"patch": {"archived": true}}),
                None,
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        // `model: null` is the explicit clear.
        let r = p
            .op(&ctx(), "s1", "sepia.meta", json!({"model": null}), None)
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        // Empty patch → 400.
        let r = p
            .op(&ctx(), "s1", "sepia.meta.patch", json!({}), None)
            .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let calls = fake.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].contains("patch_meta:s1:"), "{calls:?}");
        assert!(calls[2].contains("model: Some(None)"), "{calls:?}");
    }

    #[tokio::test]
    async fn op_convert_target_and_from() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p
            .op(
                &ctx(),
                "s1",
                "sepia.convert",
                json!({"agent": "cline", "from": "devin"}),
                None,
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_json(r).await;
        assert_eq!(body["sessionId"], "converted-1");
        assert_eq!(body["agentId"], "cline");
        assert_eq!(fake.calls(), vec!["convert:s1:cline:devin"]);
    }

    #[tokio::test]
    async fn op_delete_falls_back_to_store_on_unknown_agent() {
        let mut fake = Fake::new();
        fake.delete_unknown_agent = true;
        let fake = Arc::new(fake);
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p
            .op(
                &ctx(),
                "s1",
                "entity.delete",
                json!({}),
                Some("agent=cursor"),
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            fake.calls(),
            vec!["delete:s1:cursor", "delete_store:cursor:s1"]
        );
    }

    #[tokio::test]
    async fn op_unknown_verb_is_501() {
        let p = plane(Fake::new());
        let r = p.op(&ctx(), "s1", "sepia.bogus", json!({}), None).await;
        assert_eq!(r.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn export_wraps_session_ir() {
        let mut fake = Fake::new();
        fake.session = sepia_core::wire::session_from_json(&json!({
            "id": "s1", "title": "T", "workingDirectory": "/w",
            "model": "m", "createdAt": 0.0, "lastActivityAt": 0.0,
            "mainChainId": 0,
        }))
        .ok();
        let fake = Arc::new(fake);
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p.export(&ctx(), "s1", Some("agent=devin")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_json(r).await;
        assert_eq!(body["session"]["id"], "s1");
        assert_eq!(fake.calls(), vec!["session:s1:devin"]);
    }

    #[tokio::test]
    async fn import_requires_agent_and_accepts_ir() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        // No agent → 400.
        let r = p.import(&ctx(), json!({"session": {"id": "x"}})).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // IR + agent + overrides → backend call.
        let r = p
            .import(
                &ctx(),
                json!({
                    "session": {"id": "x", "title": "T", "workingDirectory": "/w",
                                "model": "m", "createdAt": 0.0, "lastActivityAt": 0.0,
                                "mainChainId": 0},
                    "agent": "devin", "id": "keep-me", "title": "Renamed",
                }),
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_json(r).await;
        assert_eq!(body["id"], "keep-me");
        assert_eq!(body["agentId"], "devin");
        let calls = fake.calls();
        assert!(calls[0].starts_with("import:devin:x:"), "{calls:?}");
        assert!(calls[0].contains("Renamed"), "{calls:?}");
    }

    #[tokio::test]
    async fn import_history_compat_form() {
        let fake = Arc::new(Fake::new());
        let p = SepiaEntityPlane::new(fake.clone());
        let r = p
            .import(
                &ctx(),
                json!({
                    "agent": "cline",
                    "history": [
                        {"role": "user", "content": "hello", "createdAt": 1000.0},
                        {"role": "assistant", "content": "hi", "createdAt": 2000.0},
                    ],
                }),
            )
            .await;
        assert_eq!(r.status(), StatusCode::OK);
        let calls = fake.calls();
        assert!(calls[0].starts_with("import:cline:"), "{calls:?}");
    }
}
