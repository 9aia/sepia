//! Public contract for the sepia control plane — the frozen seam between
//! `sepia-control` and `sepia-http`/`sepia-node`.

use std::path::{Path, PathBuf};

use sepia_acp::{AcpCapabilities, AcpConnection};
use sepia_core::rewind::RewindPlan;
use sepia_core::{Block, Session, TokenUsage, ToolCallDiff, ToolCallLocation, ToolCallStatus};
use serde::{Deserialize, Serialize};

/// One run span: which agent on which Sepia node continued a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSpan {
    /// Epoch milliseconds when the span was recorded (attach time).
    pub at: f64,
    pub agent: String,
    pub node: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub agent: String,
    pub updated_at: String,
    pub locked: bool,
    pub lock_holder_pid: Option<f64>,
    pub source: String,
    pub busy: bool,
    /// Sepia-overlay metadata (not part of the agent's own store).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<String>>,
    /// Run provenance from the meta overlay; empty until the first attach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spans: Option<Vec<RunSpan>>,
    /// Id of the session that spawned this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Sub-agent identity within the parent's team (not the runtime).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    pub role: sepia_core::Role,
    /// The node this message came from — the key a conversation rewind
    /// truncates after.
    pub node_id: i64,
    pub content: String,
    /// Present only when the store recorded non-text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Block>>,
    /// Epoch milliseconds.
    pub created_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Opaque provider seal on `thinking` — replayed verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Tool-result messages only: how the call this answers ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_status: Option<ToolCallStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Tool-result messages only: the call's raw input args, JSON-encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Tool-result messages only: the call's file footprint, joined by
    /// `tool_call_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<ToolCallLocation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diffs: Option<Vec<ToolCallDiff>>,
    /// Tool-result messages only: the call this message answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttachResult {
    pub attached: bool,
    pub read_only: bool,
    /// The agent runtime the session is (or would be) attached under.
    pub agent_id: String,
    /// The agent's `initialize` capability advertisement.
    pub capabilities: AcpCapabilities,
}

/// A file-level restore — `path` (+ optional `tool_call_id`) reverts
/// through recorded diffs; `checkpoint` materializes a snapshot ref.
/// `confirm` is mandatory.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub confirm: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredFile {
    /// Absolute path the restore touched.
    pub path: String,
    pub action: RestoreAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestoreAction {
    Written,
    Deleted,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestoreResult {
    pub restored: Vec<RestoredFile>,
    pub skipped: Vec<SkippedFile>,
}

/// A conversation rewind — exactly one selector; `confirm` mandatory.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindRequest {
    pub confirm: bool,
    #[serde(default)]
    pub node_id: Option<i64>,
    #[serde(default)]
    pub turns: Option<i64>,
    #[serde(default)]
    pub checkpoint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RewindResult {
    /// Nodes that survived the cut.
    pub kept: usize,
    /// Nodes the cut dropped.
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPage {
    pub messages: Vec<HistoryMessage>,
    pub total: usize,
    /// Absolute index of `messages[0]`; `> 0` means earlier history exists.
    pub start: usize,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryOptions {
    /// Trailing message count; defaults to `SEPIA_HISTORY_LIMIT`.
    pub limit: Option<usize>,
    /// Exclusive end index.
    pub before: Option<i64>,
    /// Resolve the id within this agent's store — ids collide across agents.
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    /// Last `initialize` advertisement — absent before the first spawn.
    pub capabilities: Option<AcpCapabilities>,
}

/// One runnable ACP agent, injected so tests can fake the spawn.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    async fn spawn(
        &self,
        cwd: &str,
        model: Option<&str>,
        fallbacks: Option<&[String]>,
    ) -> Result<AcpConnection, sepia_acp::rpc::RpcError>;
}

/// The store-specific write behind `rewind`, injected per backend so the
/// control plane stays store-agnostic.
#[async_trait::async_trait]
pub trait SessionRewinder: Send + Sync {
    async fn truncate(
        &self,
        session: &Session,
        plan: &RewindPlan,
        truncated: &Session,
    ) -> Result<(), ControlError>;
}

/// A `git` subprocess result — a non-zero exit is a valid answer, not an
/// error (the error path is for spawn failures).
#[derive(Clone, Debug)]
pub struct GitResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// The filesystem/git seam `restore` works through — injectable so tests
/// (and embedders) can fake the disk.
#[async_trait::async_trait]
pub trait RestoreExec: Send + Sync {
    /// File bytes, or `None` when absent.
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, String>;
    /// Write, creating parent directories.
    async fn write_file(&self, path: &Path, content: &[u8]) -> Result<(), String>;
    async fn remove_file(&self, path: &Path) -> Result<(), String>;
    async fn git(&self, cwd: &Path, args: &[&str]) -> Result<GitResult, String>;
}

/// Errors the control plane produces — `code` is the wire taxonomy.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
    pub cause: Option<String>,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    pub fn caused(
        code: ControlErrorCode,
        message: impl Into<String>,
        cause: impl std::fmt::Display,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            cause: Some(cause.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    NotFound,
    Invalid,
    Locked,
    UnknownAgent,
    Conflict,
    Busy,
    Internal,
}

impl ControlErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Invalid => "invalid",
            Self::Locked => "locked",
            Self::UnknownAgent => "unknown_agent",
            Self::Conflict => "conflict",
            Self::Busy => "busy",
            Self::Internal => "internal",
        }
    }
}

#[derive(Default)]
pub struct ControlPlaneOptions {
    pub agents: Vec<std::sync::Arc<dyn AgentRuntime>>,
    pub default_agent_id: Option<String>,
    /// Directory the lock probe runs in; defaults to cwd.
    pub probe_cwd: Option<PathBuf>,
    /// Detach a live session after this long with no listeners and no
    /// turn; `None` disables.
    pub idle_ttl: Option<std::time::Duration>,
    /// How often the idle sweep runs.
    pub sweep_interval: Option<std::time::Duration>,
    /// A pooled lock-probe connection unused this long is retired.
    pub probe_idle: Option<std::time::Duration>,
    /// Signals the lock-holder pid during an explicit takeover — SIGTERM
    /// by default.
    pub terminate_lock_holder: Option<std::sync::Arc<dyn Fn(i64) + Send + Sync>>,
    /// The disk/git seam for `restore`.
    pub restore_exec: Option<std::sync::Arc<dyn RestoreExec>>,
    /// `<dir>/<sessionId>/<backupName>` — Claude's file-history root.
    pub file_history_dir: Option<PathBuf>,
    /// Per-backend transcript writers for `rewind`.
    pub rewinders: std::collections::HashMap<String, std::sync::Arc<dyn SessionRewinder>>,
}
