//! sepia-convert — cross-store session conversion: rebuild the IR from a
//! flattened history projection, import an IR into the Devin store with
//! cog scaffolding grafted from a sibling session.

use std::sync::Arc;

use sepia_core::storage::SessionRepository;
use sepia_core::{
    Block, MessageNode, PromptHistoryEntry, Role, Session, TokenUsage, ToolCallStatus,
    ToolResultInfo,
};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ConversionError {
    pub message: String,
    pub cause: Option<String>,
}

impl ConversionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }
}

fn storage_err(prefix: &str) -> impl Fn(sepia_core::StorageError) -> ConversionError + '_ {
    move |e| ConversionError {
        message: format!("{prefix}: {}", e.message),
        cause: Some(e.message),
    }
}

/// One flattened IR message — the wire shape of the history projection.
/// The projection drops tool-call ids, so nodes re-chain linearly.
#[derive(Clone, Debug)]
pub struct ImportedHistoryMessage {
    pub role: Role,
    pub content: String,
    /// Non-text content (images, attachments) plus text blocks — `content`
    /// is the whole message otherwise.
    pub blocks: Option<Vec<Block>>,
    /// Epoch milliseconds.
    pub created_at: f64,
    pub tool_name: Option<String>,
    pub thinking: Option<String>,
    /// Opaque provider seal — replayed verbatim.
    pub thinking_signature: Option<String>,
    pub usage: Option<TokenUsage>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub finish_reason: Option<String>,
    /// Tool-result nodes only: how the call this answers ended.
    pub tool_status: Option<ToolCallStatus>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<f64>,
}

fn history_message_seconds(ms: f64, fallback: f64) -> f64 {
    if ms.is_finite() && ms > 0.0 {
        (ms / 1000.0).floor()
    } else {
        fallback
    }
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Rebuild a `Session` IR from the flattened message list the history
/// endpoint serves. What survives (roles, text, thinking + signature,
/// tool output, timestamps) is exactly what a portable resume needs.
pub fn session_from_history(
    id: &str,
    title: &str,
    cwd: &str,
    model: &str,
    history: &[ImportedHistoryMessage],
) -> Session {
    let now = now_seconds().floor();
    let nodes: Vec<MessageNode> = history
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            MessageNode {
                node_id: index,
                parent_node_id: (index > 0).then_some(index - 1),
                role: message.role,
                content: message.content.clone(),
                blocks: message.blocks.clone().unwrap_or_default(),
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: message
                    .tool_name
                    .as_ref()
                    .filter(|t| !t.is_empty())
                    .cloned(),
                thinking: message.thinking.as_ref().filter(|t| !t.is_empty()).cloned(),
                thinking_signature: message.thinking_signature.clone(),
                usage: message.usage.clone(),
                model: message.model.clone(),
                request_id: message.request_id.clone(),
                finish_reason: message.finish_reason.clone(),
                tool_result: message.tool_status.map(|status| ToolResultInfo {
                    status,
                    exit_code: message.exit_code,
                    duration_ms: message.duration_ms,
                }),
                created_at: history_message_seconds(message.created_at, now),
                metadata: match message.role {
                    Role::Assistant => serde_json::json!({
                        "summarized_from": null,
                        "num_tokens_preceding": null,
                        "is_system_prefix": null
                    }),
                    Role::System => serde_json::json!({
                        "summarized_from": null,
                        "num_tokens_preceding": null,
                        "is_system_prefix": index == 0
                    }),
                    _ => Value::Null,
                },
            }
        })
        .collect();
    let created_at = nodes.first().map_or(now, |n| n.created_at);
    let last_activity_at = nodes.last().map_or(created_at, |n| n.created_at);
    let prompt_history: Vec<PromptHistoryEntry> = history
        .iter()
        .filter(|m| m.role == Role::User)
        .map(|m| PromptHistoryEntry {
            content: m.content.clone(),
            timestamp: if m.created_at.is_finite() {
                m.created_at
            } else {
                now * 1000.0
            },
            is_shell: false,
        })
        .collect();
    Session {
        id: id.into(),
        title: title.into(),
        working_directory: cwd.into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: model.into(),
        created_at,
        last_activity_at,
        main_chain_id: i64::try_from(nodes.len().saturating_sub(1)).unwrap_or(i64::MAX),
        shell_last_seen_index: 0,
        cogs_json: sepia_core::shared::default_cogs_json(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: vec![],
        metadata: sepia_core::shared::default_session_metadata(),
        nodes,
        prompt_history,
    }
}

/// A cogs blob is usable when its `core/model` cog resolves to a real
/// model.
fn model_cog(cogs_json: &str) -> bool {
    let Ok(cogs) = serde_json::from_str::<Value>(cogs_json) else {
        return false;
    };
    cogs.as_array().is_some_and(|arr| {
        arr.iter().any(|c| {
            c["lifetime"]["Unique"] == "core/model"
                && c["model"].as_str().is_some_and(|m| !m.is_empty())
        })
    })
}

/// Fill the stub `core/model` cog's model when no donor session exists.
fn with_model(cogs_json: &str, model: &str) -> String {
    let mut cogs = serde_json::from_str::<Value>(cogs_json).unwrap_or(Value::Null);
    if let Some(arr) = cogs.as_array_mut() {
        for c in arr.iter_mut() {
            if c["lifetime"]["Unique"] == "core/model" {
                c["model"] = Value::String(model.to_string());
            }
        }
        return serde_json::to_string(&cogs).unwrap_or_else(|_| cogs_json.to_string());
    }
    cogs_json.to_string()
}

/// Sepia cannot mint Devin's cog scaffolding (model cog, tool allow-list,
/// profile prompt), so graft it from a sibling session in the store —
/// same working directory preferred — or at least fill the model cog.
async fn graft_cogs(
    repo: &Arc<dyn SessionRepository>,
    session: &Session,
) -> Result<String, ConversionError> {
    let donors = repo.list().await.unwrap_or_default();
    let usable = |s: &Session| s.id != session.id && model_cog(&s.cogs_json);
    let donor = donors
        .iter()
        .find(|s| usable(s) && s.working_directory == session.working_directory)
        .or_else(|| donors.iter().find(|s| usable(s)));
    match donor {
        Some(donor) => Ok(donor.cogs_json.clone()),
        None => Ok(with_model(&session.cogs_json, &session.model)),
    }
}

/// Save a session into the Devin store, grafting cog scaffolding from a
/// donor session when the session's own cogs carry no usable model.
/// Sessions that already exist are left untouched (returns the id
/// unchanged).
///
/// # Errors
/// On store failure.
pub async fn import_session(
    repo: &Arc<dyn SessionRepository>,
    session: &Session,
) -> Result<String, ConversionError> {
    if repo
        .has_session(&session.id)
        .await
        .map_err(storage_err("Import failed"))?
    {
        return Ok(session.id.clone());
    }
    let cogs_json = graft_cogs(repo, session).await?;
    repo.save(&Session {
        cogs_json,
        ..session.clone()
    })
    .await
    .map_err(storage_err("Import failed"))?;
    Ok(session.id.clone())
}

/// Read a Cline session dir into the IR and import it into the Devin
/// store. Existing sessions are left untouched.
///
/// # Errors
/// `ConversionError` on parse/store failure.
pub async fn import_cline(
    cline_dir: &std::path::Path,
    session_id: Option<&str>,
    repo: &Arc<dyn SessionRepository>,
    dry_run: bool,
) -> Result<String, ConversionError> {
    let mut session =
        sepia_driver_cline::cline::from_directory(cline_dir, session_id).map_err(|e| {
            ConversionError {
                message: format!("Import failed: {}", e.message),
                cause: e.cause.as_str().map(str::to_string),
            }
        })?;
    session.backend_type = "cline".into();
    let stored_id = session.id.clone();
    if repo
        .has_session(&stored_id)
        .await
        .map_err(storage_err("Import failed"))?
    {
        return Ok(stored_id);
    }
    if dry_run {
        return Ok(stored_id);
    }
    import_session(repo, &session).await
}

/// Write a stored session out to a Cline session dir (manifest +
/// transcript pair).
///
/// # Errors
/// `ConversionError` on unknown session or write failure.
pub async fn export_cline(
    repo: &Arc<dyn SessionRepository>,
    session_id: &str,
    out_dir: &std::path::Path,
    force: bool,
    dry_run: bool,
) -> Result<(), ConversionError> {
    let session = repo
        .get_by_id(session_id, None)
        .await
        .map_err(storage_err("Export failed"))?
        .ok_or_else(|| ConversionError::new(format!("Session not found: {session_id}")))?;
    sepia_driver_cline::cline::to_directory(&session, out_dir, force, dry_run).map_err(|e| {
        ConversionError {
            message: format!("Export failed: {}", e.message),
            cause: e.cause.as_str().map(str::to_string),
        }
    })?;
    Ok(())
}

/// Export a Devin session straight into the Cline CLI store: artifacts
/// land in `<data_dir>/sessions/<id>/` and the index row is registered,
/// so `cline --id <id>` resumes it. `force` overwrites a live-owned row.
///
/// # Errors
/// `ConversionError` on unknown session or store write failure.
pub async fn install_cline(
    repo: &Arc<dyn SessionRepository>,
    session_id: &str,
    data_dir: &std::path::Path,
    new_session_id: Option<&str>,
    force: bool,
) -> Result<String, ConversionError> {
    let session = repo
        .get_by_id(session_id, None)
        .await
        .map_err(storage_err("Install failed"))?
        .ok_or_else(|| ConversionError::new(format!("Session not found: {session_id}")))?;
    let id = new_session_id.map_or_else(
        || sepia_driver_cline::cline::cline_session_id(session.created_at * 1000.0),
        str::to_string,
    );
    let store = sepia_driver_cline::ClineStore::new(data_dir.to_path_buf());
    store
        .install(&session, &id, force)
        .map_err(|e| ConversionError {
            message: format!("Install failed: {}", e.message),
            cause: Some(e.message.clone()),
        })?;
    Ok(id)
}
