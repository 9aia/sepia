//! Session IR — the normalized transcript shape every store adapter reads
//! and writes. Wire semantics match the legacy `SessionJson` payload:
//! optional fields ride as `field?: value` (absent = none), defaulted
//! fields are emitted always but decode absent → default.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallStatus {
    Pending,
    Success,
    Error,
}

/// Token metrics for one message. `input`/`output` are the counts every
/// store carries; the rest appear only where the agent persists them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// Outcome a `role: "tool"` node reports back for the call it answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultInfo {
    pub status: ToolCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}

/// A file location a tool call touched (ACP `locations`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
}

/// The before/after payload a store recorded for a file change.
/// Absent `old_text` = create, absent `new_text` = delete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDiff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
}

/// A snapshot pointer a store records for workspace state — a reference,
/// never a payload. `created_at` is epoch **milliseconds**.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRef {
    pub r#ref: String,
    pub created_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

fn default_tool_kind() -> String {
    "function".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub index: i64,
    #[serde(default = "default_tool_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Files the call touched, when the store recorded them.
    #[serde(default)]
    pub locations: Vec<ToolCallLocation>,
    /// File changes the call made, when the store recorded before/after
    /// payloads.
    #[serde(default)]
    pub diffs: Vec<ToolCallDiff>,
}

/// One piece of message content beyond the flat `content` string. When
/// `blocks` is populated it holds the complete ordered block list — text
/// blocks included — so `content` stays the joined text projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Block {
    Text {
        text: String,
    },
    /// `data` is base64; `uri` covers linked (not embedded) images.
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
    Audio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// A file the message references (`uri`/`name`) or embeds
    /// (`text`/`data`, base64 for `data`).
    File {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
}

/// Display text recorded in `thinking` when a store held reasoning it
/// cannot show (Cursor `redacted-reasoning`, Claude `redacted_thinking`).
pub const REDACTED_THINKING: &str = "[redacted]";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptHistoryEntry {
    pub content: String,
    pub timestamp: f64,
    #[serde(default)]
    pub is_shell: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageNode {
    pub node_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_node_id: Option<i64>,
    pub role: Role,
    pub content: String,
    /// Complete ordered block list when the store recorded non-text
    /// content; `content` remains the joined text projection.
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Opaque provider seal for `thinking` — preserved verbatim, never
    /// decoded. When several sealed blocks fold into one node the last
    /// signature wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Model that generated this message; session-level `model` is default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// On `role: "tool"` nodes: how the call this result answers ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultInfo>,
    pub created_at: f64,
    #[serde(default)]
    pub metadata: Value,
}

fn default_backend_type() -> String {
    "windsurf".into()
}
fn default_agent_mode() -> String {
    "accept-edits".into()
}
fn default_empty_json_array() -> String {
    "[]".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub working_directory: String,
    #[serde(default = "default_backend_type")]
    pub backend_type: String,
    #[serde(default = "default_agent_mode")]
    pub agent_mode: String,
    pub model: String,
    pub created_at: f64,
    pub last_activity_at: f64,
    pub main_chain_id: i64,
    #[serde(default)]
    pub shell_last_seen_index: i64,
    #[serde(default = "default_empty_json_array")]
    pub cogs_json: String,
    #[serde(default = "default_empty_json_array")]
    pub workspace_dirs: String,
    #[serde(default)]
    pub hidden: i64,
    /// The session that spawned this one, when the store records a
    /// sub-agent tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Sub-agent identity within the parent session's team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Workspace snapshot refs the store recorded. References only.
    #[serde(default)]
    pub checkpoints: Vec<CheckpointRef>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub nodes: Vec<MessageNode>,
    #[serde(default)]
    pub prompt_history: Vec<PromptHistoryEntry>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct StorageError {
    pub message: String,
}

impl StorageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ConversionError {
    pub message: String,
    pub cause: Value,
}
