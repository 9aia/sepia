//! `SessionEvent` — the live-session SSE payload stream (protocol v2).
//! Carries what AG-UI carried, minus the framework: run lifecycle,
//! text/reasoning message frames, tool-call frames, and a `custom` escape
//! hatch for ACP-specific payloads (`acp:plan`, `acp:permission_request`,
//! mid-call file updates, unknown update kinds).

use sepia_core::{ToolCallDiff, ToolCallLocation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionEvent {
    RunStarted {
        thread_id: String,
        run_id: String,
    },
    TextMessageStart {
        message_id: String,
        role: String,
    },
    TextMessageContent {
        message_id: String,
        delta: String,
    },
    TextMessageEnd {
        message_id: String,
    },
    ReasoningMessageStart {
        message_id: String,
        role: String,
    },
    ReasoningMessageContent {
        message_id: String,
        delta: String,
    },
    ReasoningMessageEnd {
        message_id: String,
    },
    ToolCallStart {
        tool_call_id: String,
        tool_call_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<ToolCallLocation>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diffs: Option<Vec<ToolCallDiff>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        contents: Option<Vec<Value>>,
    },
    ToolCallArgs {
        tool_call_id: String,
        delta: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_name: Option<String>,
    },
    ToolCallResult {
        message_id: String,
        tool_call_id: String,
        content: String,
    },
    ToolCallEnd {
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<ToolCallLocation>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diffs: Option<Vec<ToolCallDiff>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        contents: Option<Vec<Value>>,
    },
    /// Mid-call file/content update — no dedicated frame, rides custom
    /// like the AG-UI stream did (`acp:tool_call_update` etc.).
    Custom {
        name: String,
        value: Value,
    },
    RunFinished {
        thread_id: String,
        run_id: String,
    },
}
