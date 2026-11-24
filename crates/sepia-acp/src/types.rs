//! Public contract for the sepia ACP runtime — the frozen seam between
//! `sepia-acp` and its consumers (control plane, UI event translation).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSpec {
    pub id: String,
    pub label: String,
    pub command: Vec<String>,
    pub env: Option<BTreeMap<String, String>>,
}

/// `agentCapabilities.promptCapabilities` flattened — ACP defaults every
/// flag to false when unadvertised.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptCapabilities {
    pub image: bool,
    pub audio: bool,
    /// Whether `resource` content blocks (embedded context) are accepted.
    pub embedded_context: bool,
}

/// `agentCapabilities.sessionCapabilities` flattened — each ACP entry is
/// an object (possibly empty) whose presence advertises the method.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionCapabilities {
    pub list: bool,
    pub delete: bool,
    pub fork: bool,
    pub resume: bool,
    pub close: bool,
    pub additional_directories: bool,
}

/// The agent's capability advertisement, captured at `initialize`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCapabilities {
    pub load_session: bool,
    /// `sessionCapabilities.list` flattened — kept for existing callers.
    pub session_list: bool,
    pub prompt_capabilities: AcpPromptCapabilities,
    pub session_capabilities: AcpSessionCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionInfo {
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub updated_at: String,
    pub locked: bool,
    pub lock_holder_pid: Option<f64>,
}

/// A non-diff `content` entry of a tool call — the ACP
/// `{type:"terminal", terminalId}` refs (with `output` when the agent
/// inlines the terminal text) and wrapped `ContentBlock`s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolCallContent {
    Terminal {
        terminal_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    Text {
        text: String,
    },
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

/// Normalized subset of ACP `session/update` notifications we render.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcpSessionUpdate {
    AgentMessageChunk {
        text: String,
    },
    AgentThoughtChunk {
        text: String,
    },
    UserMessageChunk {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        status: String,
        tool_kind: String,
        raw_input: Value,
        locations: Vec<sepia_core::ToolCallLocation>,
        diffs: Vec<sepia_core::ToolCallDiff>,
        #[serde(skip_serializing_if = "Option::is_none")]
        contents: Option<Vec<ToolCallContent>>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<sepia_core::ToolCallLocation>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diffs: Option<Vec<sepia_core::ToolCallDiff>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        contents: Option<Vec<ToolCallContent>>,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    CurrentModeUpdate {
        mode_id: String,
    },
    AvailableCommandsUpdate {
        commands: Vec<AvailableCommand>,
    },
    Other {
        session_update: String,
        raw: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableCommandInput {
    /// Placeholder hint for the command's arguments (`<file>`, `[msg]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableCommand {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `input` — argument shape the command accepts (hint only today).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<AvailableCommandInput>,
    /// Vendor extension payload (`_meta`) — retained verbatim.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub tool_call_id: Option<String>,
    /// The ACP tool-call `kind` (`execute`, `edit`, `read`, …) when the
    /// agent reported one — the handle permission policies match on.
    pub tool_kind: Option<String>,
    pub title: String,
    pub options: Vec<PermissionOption>,
}

/// A content block of an ACP `session/prompt` request — the subset of the
/// schema's `ContentBlock` union sepia sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptPart {
    Text {
        text: String,
    },
    Image {
        data: String,
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
    Audio {
        data: String,
        mime_type: String,
    },
    Resource {
        resource: PromptResource,
    },
    ResourceLink {
        uri: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PromptResource {
    Text {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        blob: String,
    },
}

/// The `session/prompt` response — resolves when the agent finishes
/// the turn. `stop_reason` is the agent's `stopReason` (`end_turn`,
/// `cancelled`, `max_tokens`, `refusal`, …); `None` when the agent
/// omitted it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptResult {
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// The callable inside [`ClientRequestHandler`] — factored out so the
/// tuple struct stays readable.
pub type ClientRequestFn =
    Arc<dyn Fn(&str, &Value) -> Result<Value, sepia_driver::rpc::RpcError> + Send + Sync>;

/// An agent→client request handler for methods outside the in-band
/// permission lane (`fs/read_text_file`, terminal ops, …). Receives
/// the method name and params; returns the JSON-RPC `result` payload
/// or an error. Runs on the blocking pool — keep it fast.
#[derive(Clone)]
pub struct ClientRequestHandler(pub ClientRequestFn);

impl std::fmt::Debug for ClientRequestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientRequestHandler")
    }
}

/// Wire-level ndjson transcript sink — every line in both directions,
/// pre-parse (non-JSON garbage included). `SEPIA_ACP_DEBUG` mirrors
/// this when `SpawnOptions::wire_debug` is unset: `1`/`true` →
/// stderr, any other truthy value → file path (appended).
#[derive(Clone, Debug)]
pub enum WireDebug {
    Stderr,
    File(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    pub cwd: String,
    pub env: Option<BTreeMap<String, String>>,
    /// Preferred model — passed to the agent's spawn flag, applies at spawn.
    pub model: Option<String>,
    /// Ordered fallback models (agent-specific flag).
    pub fallbacks: Option<Vec<String>>,
    /// Forward the whole parent environment (default: the
    /// `ALLOWED_ENV_KEYS` allowlist only). Unattended consumers whose
    /// agents need arbitrary auth/config vars opt in;
    /// `SEPIA_INHERIT_ENV=1` forces it process-wide.
    pub inherit_env: bool,
    /// Extra `clientCapabilities` merged into `initialize` — e.g.
    /// `{"fs":{"readTextFile":true}}` when a `request_handler` serves
    /// fs reads.
    pub client_capabilities: Option<Value>,
    /// `clientInfo` reported at `initialize`.
    pub client_info: Option<Value>,
    /// Handler for agent→client requests other than
    /// `session/request_permission` — an unanswered request stalls
    /// the agent, so set one whenever the agent may call back. Methods
    /// with no handler get a `-32601` response.
    pub request_handler: Option<ClientRequestHandler>,
    /// ndjson transcript sink; `SEPIA_ACP_DEBUG` is the env fallback.
    pub wire_debug: Option<WireDebug>,
}
