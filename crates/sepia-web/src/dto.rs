//! Wire DTOs mirroring `sepia-http`'s response shapes. These are
//! deliberately tolerant (`#[serde(default)]` everywhere, `Value` for
//! blocks) so a newer node can add fields without breaking an older
//! hub — and so one malformed message can't poison a whole page.

use sepia_core::{TokenUsage, ToolCallDiff, ToolCallLocation};
use sepia_web_core::filter::SessionRow;
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
    /// Never part of the plain summary GET — it arrives via the feed's
    /// `{live: bool}` session patch (merged into the hub projection's
    /// raw row), so absence reads as `false`.
    #[serde(default)]
    pub live: bool,
    /// Run provenance — each attach appends a `{at, agent, node}` span.
    /// Rides `SummaryWire.spans`; empty until the first attach.
    #[serde(default)]
    pub spans: Vec<RunSpanDto>,
    /// Hub-side annotation — the node id that owns this session.
    /// Not part of the node's wire summary; absent → `None`.
    #[serde(default)]
    pub node: Option<String>,
}

/// `SummaryWire.spans[]` — `sepia_meta::RunSpan`/`sepia_control::RunSpan`
/// on the wire (`{at, agent, node}`, `at` epoch ms).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSpanDto {
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub node: String,
}

impl From<&RunSpanDto> for sepia_web_core::history::RunSpan {
    fn from(s: &RunSpanDto) -> Self {
        Self {
            at: s.at,
            agent: s.agent.clone(),
            node: s.node.clone(),
        }
    }
}

/// The list view's row — `sepia-web-core` runs the whole
/// filter/sort/group pipeline on it.
impl From<&SessionSummaryDto> for SessionRow {
    fn from(s: &SessionSummaryDto) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            cwd: s.cwd.clone(),
            agent: s.agent.clone(),
            updated_at: s.updated_at.clone(),
            locked: s.locked,
            live: s.live,
            busy: s.busy,
            pinned: s.pinned,
            archived: s.archived,
            parent_session_id: s.parent_session_id.clone(),
            node: s.node.clone(),
        }
    }
}

/// `POST /api/sessions` → `CreateResultWire` (`{id, agentId,
/// capabilities}` — the capabilities object is ignored here).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResultDto {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
}

/// `POST /api/sessions/{id}/attach` → `AttachResultWire`
/// (`{attached, readOnly, agentId, capabilities}`). `read_only` means a
/// foreign process holds the store lock.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachResultDto {
    #[serde(default)]
    pub attached: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub agent_id: String,
}

/// `GET /api/sessions/{id}/checkpoints` row — `sepia_core::CheckpointRef`
/// (`{ref, createdAt, runCount?, kind?}`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointDto {
    #[serde(default)]
    pub r#ref: String,
    /// Epoch milliseconds.
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub run_count: Option<i64>,
    #[serde(default)]
    pub kind: Option<String>,
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
    /// Token metrics the store recorded for this message
    /// (`sepia_core::TokenUsage` — `input`/`output` + optional
    /// `cacheRead`/`cacheWrite`/`thinking`/`cost`).
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    /// Structured content (`sepia_core::Block`, tagged `"type"`). Kept
    /// as `Value` so unknown block kinds degrade rather than fail.
    #[serde(default)]
    pub blocks: Option<Vec<Value>>,
    /// Tool-result rows only: files the call touched, joined by
    /// `tool_call_id`.
    #[serde(default)]
    pub locations: Option<Vec<ToolCallLocation>>,
    /// Tool-result rows only: recorded before/after payloads.
    #[serde(default)]
    pub diffs: Option<Vec<ToolCallDiff>>,
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

/// `GET /api/agents` row — `sepia_http::routes::AgentWire`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDto {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub capabilities: Option<AgentCapabilitiesDto>,
    /// Hub-side annotation — the node that advertises this agent
    /// (multi-node hubs only; absent → `None`).
    #[serde(default)]
    pub node: Option<String>,
}

/// `AgentWire.capabilities` — `sepia_http::routes::CapabilitiesWire`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilitiesDto {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub session_list: bool,
    #[serde(default)]
    pub prompt_capabilities: PromptCapabilitiesDto,
    #[serde(default)]
    pub session_capabilities: SessionCapabilitiesDto,
}

/// `CapabilitiesWire.promptCapabilities` — `AcpPromptCapabilities`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCapabilitiesDto {
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub embedded_context: bool,
}

/// `CapabilitiesWire.sessionCapabilities` — `AcpSessionCapabilities`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilitiesDto {
    #[serde(default)]
    pub list: bool,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub resume: bool,
    #[serde(default)]
    pub close: bool,
    #[serde(default)]
    pub additional_directories: bool,
}

/// `GET /api/projects` row — `sepia_meta::Project` on the wire.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Hub-side annotation — the node owning this project (multi-node
    /// hubs only; absent → `None`).
    #[serde(default)]
    pub node: Option<String>,
}

/// `GET /api/node` — the node descriptor (`routes/node.ts` port).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfoDto {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// One row of hub-side node health — `sepia_sync::NodeRow` under the
/// sync engine, or a synthesized single-row probe from `HttpNodeApi`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatusDto {
    pub id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub label: String,
    /// `"up" | "down" | "unknown"`.
    #[serde(default)]
    pub status: String,
    /// RFC 3339 — last successful contact (sync engines only).
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

/// One row of the hub's durable write queue — `sepia_outbox`'s
/// `pending_for_node` + `dead_letters` flattened for the UI. Only
/// sync-engine hubs have an outbox; a single-node `HttpNodeApi` returns
/// `[]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWriteDto {
    pub id: String,
    /// The node the write is queued for.
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub session_id: String,
    /// Free-form op name (`prompt`, `cancel`, `meta.patch`, …).
    #[serde(default)]
    pub op: String,
    /// `"metadata" | "turn"` — the replay conflict policy.
    #[serde(default)]
    pub kind: String,
    /// `"queued"` (pending/in-flight) or `"failed"` (dead-lettered).
    #[serde(default)]
    pub status: String,
    /// RFC 3339 — when the write was queued.
    #[serde(default)]
    pub enqueued_at: String,
    #[serde(default)]
    pub attempts: i64,
    /// The last replay error — what dead-letters failed with.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// `POST /api/push/subscribe` body — `{endpoint, keys:{auth,p256dh}}`
/// (`prefs` left at the node's default).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscriptionDto {
    pub endpoint: String,
    pub keys: PushKeysDto,
}

/// The `keys` sub-object of a push subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushKeysDto {
    pub auth: String,
    pub p256dh: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_write_serializes_with_wire_names() {
        let dto = PendingWriteDto {
            id: "w1".into(),
            node_id: "tower".into(),
            session_id: "s1".into(),
            op: "prompt".into(),
            kind: "turn".into(),
            status: "failed".into(),
            enqueued_at: "2026-10-08T06:40:34.123Z".into(),
            attempts: 5,
            last_error: Some("node unreachable".into()),
        };
        let v = serde_json::to_value(&dto).unwrap_or_default();
        assert_eq!(v["nodeId"], "tower");
        assert_eq!(v["sessionId"], "s1");
        assert_eq!(v["enqueuedAt"], "2026-10-08T06:40:34.123Z");
        assert_eq!(v["lastError"], "node unreachable");
        // Tolerant decode — a missing field defaults, extra fields drop.
        let parsed: PendingWriteDto =
            serde_json::from_str(r#"{"id":"w2","op":"cancel","extra":1}"#).unwrap_or_default();
        assert_eq!(parsed.id, "w2");
        assert_eq!(parsed.status, "");
        assert_eq!(parsed.attempts, 0);
    }
}
