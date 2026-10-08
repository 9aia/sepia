//! Wire DTOs mirroring `sepia-http`'s response shapes. These are
//! deliberately tolerant (`#[serde(default)]` everywhere, `Value` for
//! blocks) so a newer node can add fields without breaking an older
//! hub — and so one malformed message can't poison a whole page.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `GET /api/sessions` row — `sepia_http::routes::SummaryWire`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummaryDto {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub lock_holder_pid: Option<f64>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// `GET /api/sessions/{id}/history` page —
/// `sepia_http::routes::HistoryPageWire`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageDto {
    #[serde(default)]
    pub messages: Vec<HistoryMessageDto>,
    #[serde(default)]
    pub total: usize,
    /// Node index the first `messages` entry occupies — the `?before`
    /// cursor for the next-earlier page.
    #[serde(default)]
    pub start: usize,
}

/// One history row — `sepia_control::HistoryMessage`'s wire form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessageDto {
    /// `"system" | "user" | "assistant" | "tool"`.
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub node_id: i64,
    #[serde(default)]
    pub content: String,
    /// Epoch milliseconds.
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    /// `"pending" | "success" | "error"` on tool-result rows.
    #[serde(default)]
    pub tool_status: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<f64>,
    /// Tool-call args, JSON-encoded.
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Structured content (`sepia_core::Block`, tagged `"type"`). Kept
    /// as `Value` so unknown block kinds degrade rather than fail.
    #[serde(default)]
    pub blocks: Option<Vec<Value>>,
}

impl HistoryMessageDto {
    /// The message's display text: `content`, or the joined `text`
    /// blocks when `content` is empty.
    pub fn text(&self) -> String {
        if !self.content.is_empty() {
            return self.content.clone();
        }
        let Some(blocks) = &self.blocks else {
            return String::new();
        };
        blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    }
}
