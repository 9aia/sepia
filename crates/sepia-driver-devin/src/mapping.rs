//! `chat_message` blob ↔ `MessageNode` — the Devin.ts port. The blobs are
//! ACP-shaped JSON the Devin CLI wrote; parsing is deliberately tolerant
//! (any malformed field degrades to its default, never fails the row).

use sepia_core::domain::{
    Block, MessageNode, PromptHistoryEntry, Role, Session, TokenUsage, ToolCall, ToolCallDiff,
    ToolCallLocation, ToolCallStatus, ToolResultInfo,
};
use sepia_core::shared::{self};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

fn to_iso(ts: f64) -> String {
    // Node `createdAt` is epoch seconds.
    match time::OffsetDateTime::from_unix_timestamp_nanos((ts * 1e9) as i128) {
        Ok(t) => t
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

fn finite_number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn num_field(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(finite_number)
}

fn obj(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

static NULL: Value = Value::Null;

fn field<'a>(m: &'a Map<String, Value>, key: &str) -> &'a Value {
    m.get(key).unwrap_or(&NULL)
}

fn present(key: &str, value: Option<impl serde::Serialize>, out: &mut Map<String, Value>) {
    if let Some(v) = value {
        if let Ok(v) = serde_json::to_value(v) {
            out.insert(key.into(), v);
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks — `chisel/acp-content-blocks`
// ---------------------------------------------------------------------------

fn block_from_acp(raw: &Value) -> Option<Block> {
    let b = obj(raw)?;
    match b.get("type").and_then(Value::as_str)? {
        "text" => Some(Block::Text {
            text: str_field(b, "text")?.to_string(),
        }),
        "image" => Some(Block::Image {
            data: str_field(b, "data").map(str::to_string),
            mime_type: str_field(b, "mimeType").map(str::to_string),
            uri: str_field(b, "uri").map(str::to_string),
        }),
        "audio" => Some(Block::Audio {
            data: str_field(b, "data").map(str::to_string),
            mime_type: str_field(b, "mimeType").map(str::to_string),
        }),
        "resource_link" => Some(Block::File {
            uri: str_field(b, "uri").map(str::to_string),
            name: str_field(b, "name")
                .or_else(|| str_field(b, "title"))
                .map(str::to_string),
            mime_type: str_field(b, "mimeType").map(str::to_string),
            size: num_field(b, "size"),
            text: None,
            data: None,
        }),
        "resource" => {
            let r = obj(b.get("resource")?)?;
            Some(Block::File {
                uri: str_field(r, "uri").map(str::to_string),
                name: None,
                mime_type: str_field(r, "mimeType").map(str::to_string),
                size: None,
                text: str_field(r, "text").map(str::to_string),
                data: str_field(r, "blob").map(str::to_string),
            })
        }
        _ => None,
    }
}

/// Kept only when a non-text block is present — an all-text list
/// duplicates `content` exactly.
pub fn blocks_from_acp(raw: &Value) -> Vec<Block> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    let blocks: Vec<Block> = items.iter().filter_map(block_from_acp).collect();
    if blocks.iter().any(|b| !matches!(b, Block::Text { .. })) {
        blocks
    } else {
        Vec::new()
    }
}

fn block_to_acp(block: &Block) -> Value {
    match block {
        Block::Text { text } => json!({ "type": "text", "text": text }),
        Block::Image {
            data,
            mime_type,
            uri,
        } => {
            let mut out = Map::new();
            out.insert("type".into(), json!("image"));
            present("data", data.as_deref(), &mut out);
            present("mimeType", mime_type.as_deref(), &mut out);
            present("uri", uri.as_deref(), &mut out);
            Value::Object(out)
        }
        Block::Audio { data, mime_type } => {
            let mut out = Map::new();
            out.insert("type".into(), json!("audio"));
            present("data", data.as_deref(), &mut out);
            present("mimeType", mime_type.as_deref(), &mut out);
            Value::Object(out)
        }
        Block::File {
            uri,
            name,
            mime_type,
            size,
            text,
            data,
        } => {
            if text.is_some() || data.is_some() {
                // ACP `resource` (embedded) has no name slot.
                let mut resource = Map::new();
                resource.insert("uri".into(), json!(uri.as_deref().unwrap_or("")));
                present("mimeType", mime_type.as_deref(), &mut resource);
                if let Some(t) = text {
                    resource.insert("text".into(), json!(t));
                } else {
                    resource.insert("blob".into(), json!(data));
                }
                json!({ "type": "resource", "resource": resource })
            } else {
                let mut out = Map::new();
                out.insert("type".into(), json!("resource_link"));
                out.insert(
                    "uri".into(),
                    json!(uri.clone().or_else(|| name.clone()).unwrap_or_default()),
                );
                out.insert(
                    "name".into(),
                    json!(name.clone().or_else(|| uri.clone()).unwrap_or_default()),
                );
                present("mimeType", mime_type.as_deref(), &mut out);
                present("size", *size, &mut out);
                Value::Object(out)
            }
        }
    }
}

fn content_blocks_extension(node: &MessageNode, out: &mut Map<String, Value>) {
    if !node.blocks.is_empty() {
        out.insert(
            "chisel/acp-content-blocks".into(),
            Value::Array(node.blocks.iter().map(block_to_acp).collect()),
        );
    }
}

// ---------------------------------------------------------------------------
// Status mapping
// ---------------------------------------------------------------------------

/// ACP tool-call status → the IR's coarser lifecycle.
pub fn from_acp_tool_call_status(status: Option<&str>) -> Option<ToolCallStatus> {
    match status? {
        "completed" => Some(ToolCallStatus::Success),
        "failed" => Some(ToolCallStatus::Error),
        "pending" | "in_progress" => Some(ToolCallStatus::Pending),
        _ => None,
    }
}

/// IR status → the ACP status Devin persists in `chisel/tool_call_content`.
pub fn to_acp_tool_call_status(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Success => "completed",
        ToolCallStatus::Error => "failed",
        ToolCallStatus::Pending => "pending",
    }
}

// ---------------------------------------------------------------------------
// Usage ↔ metrics blob
// ---------------------------------------------------------------------------

fn usage_from_metrics(metrics: &Value) -> Option<TokenUsage> {
    let m = obj(metrics)?;
    let input = num_field(m, "input_tokens");
    let output = num_field(m, "output_tokens");
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(TokenUsage {
        input: input.unwrap_or(0.0),
        output: output.unwrap_or(0.0),
        cache_read: num_field(m, "cache_read_tokens"),
        cache_write: num_field(m, "cache_creation_tokens"),
        thinking: None,
        cost: None,
    })
}

fn metrics_blob(node: &MessageNode) -> Value {
    match &node.usage {
        None => Value::Null,
        Some(u) => json!({
            "input_tokens": u.input,
            "output_tokens": u.output,
            "cache_read_tokens": u.cache_read,
            "cache_creation_tokens": u.cache_write,
        }),
    }
}

// ---------------------------------------------------------------------------
// build_chat_message — IR node → devin blob (save path)
// ---------------------------------------------------------------------------

fn tool_call_display(tc: &ToolCall) -> (String, &'static str, Value) {
    let arg_path = tc.arguments["file_path"].as_str().unwrap_or("").to_string();
    match tc.name.as_str() {
        "read" => ("Read file".into(), "read", json!([{ "path": arg_path }])),
        "exec" => ("Ran command".into(), "execute", json!([])),
        "grep" => ("Searched codebase".into(), "search", json!([])),
        "webfetch" => ("Fetched web content".into(), "fetch", json!([])),
        "edit" => ("Edited file".into(), "edit", json!([{ "path": arg_path }])),
        "write" => ("Wrote file".into(), "edit", json!([{ "path": arg_path }])),
        _ => (tc.name.clone(), "function", json!([])),
    }
}

pub fn build_chat_message(node: &MessageNode, generation_model: &str) -> Value {
    let message_id = uuid::Uuid::new_v4().to_string();
    let mut base = Map::new();
    base.insert("message_id".into(), json!(message_id));
    base.insert(
        "role".into(),
        json!(serde_json::to_value(node.role).unwrap_or_default()),
    );
    base.insert("content".into(), json!(node.content));

    match node.role {
        Role::System => Value::Object(base),
        Role::User => {
            let mut extensions = Map::new();
            content_blocks_extension(node, &mut extensions);
            base.insert(
                "metadata".into(),
                json!({
                    "num_tokens": null,
                    "is_user_input": true,
                    "request_id": null,
                    "metrics": null,
                    "finish_reason": null,
                    "extensions": extensions,
                    "created_at": to_iso(node.created_at),
                    "telemetry": { "source": "user", "operation": "input" },
                }),
            );
            Value::Object(base)
        }
        Role::Assistant => {
            let is_rendered = node.metadata.as_object().is_some_and(|m| {
                m.contains_key("summarized_from") && m["is_system_prefix"] != true
            });

            let mut extensions = Map::new();
            content_blocks_extension(node, &mut extensions);
            if is_rendered && !node.tool_calls.is_empty() {
                let mut ext = Map::new();
                for tc in &node.tool_calls {
                    let (title, kind, locations) = tool_call_display(tc);
                    let mut entry = Map::new();
                    entry.insert("toolCallId".into(), json!(tc.id));
                    entry.insert("title".into(), json!(title));
                    entry.insert(
                        "status".into(),
                        json!(to_acp_tool_call_status(
                            tc.status.unwrap_or(ToolCallStatus::Success)
                        )),
                    );
                    entry.insert(
                        "locations".into(),
                        if tc.locations.is_empty() {
                            locations
                        } else {
                            serde_json::to_value(&tc.locations).unwrap_or_default()
                        },
                    );
                    entry.insert("kind".into(), json!(kind));
                    entry.insert("rawInput".into(), tc.arguments.clone());
                    if !tc.diffs.is_empty() {
                        entry.insert(
                            "content".into(),
                            Value::Array(
                                tc.diffs
                                    .iter()
                                    .map(|d| {
                                        let mut c = Map::new();
                                        c.insert("type".into(), json!("diff"));
                                        c.insert("path".into(), json!(d.path));
                                        if let Some(old) = &d.old_text {
                                            c.insert("oldText".into(), json!(old));
                                        }
                                        if let Some(new) = &d.new_text {
                                            c.insert("newText".into(), json!(new));
                                        }
                                        Value::Object(c)
                                    })
                                    .collect(),
                            ),
                        );
                    }
                    ext.insert(tc.id.clone(), Value::Object(entry));
                }
                extensions.insert("chisel/tool_call_content".into(), Value::Object(ext));
            }

            base.insert(
                "tool_calls".into(),
                serde_json::to_value(&node.tool_calls).unwrap_or_default(),
            );
            base.insert(
                "metadata".into(),
                json!({
                    "num_tokens": node.usage.as_ref().map_or(Value::Null, |u| json!(u.output)),
                    "is_user_input": null,
                    "request_id": node.request_id,
                    "metrics": metrics_blob(node),
                    "finish_reason": node.finish_reason.clone().unwrap_or_else(|| {
                        if node.tool_calls.is_empty() { "stop" } else { "tool_calls" }.to_string()
                    }),
                    "extensions": extensions,
                    "generation_model": node.model.clone().unwrap_or_else(|| generation_model.to_string()),
                    "created_at": to_iso(node.created_at),
                    "telemetry": { "source": "assistant", "operation": "inference" },
                }),
            );
            // Unsigned thinking is dropped — the backend rejects replayed
            // blocks without a signature.
            if let Some(signature) = &node.thinking_signature {
                base.insert(
                    "thinking".into(),
                    json!({
                        "thinking": node.thinking.clone().unwrap_or_default(),
                        "signature": signature,
                    }),
                );
            }
            Value::Object(base)
        }
        Role::Tool => {
            let tool_name = node.tool_name.clone().unwrap_or_else(|| "unknown".into());
            let result = &node.tool_result;
            let mut extensions = Map::new();
            content_blocks_extension(node, &mut extensions);
            extensions.insert(
                "chisel/tool_result_meta".into(),
                json!({
                    "success": result.as_ref().is_none_or(|r| r.status != ToolCallStatus::Error),
                    "kind": tool_name,
                }),
            );
            if let Some(code) = result.as_ref().and_then(|r| r.exit_code) {
                extensions.insert(
                    "chisel/terminal_output".into(),
                    json!({ "exit": { "exit_code": code } }),
                );
            }
            if let Some(ms) = result.as_ref().and_then(|r| r.duration_ms) {
                extensions.insert(
                    "chisel/tool_call_timing".into(),
                    json!({ "duration_ms": ms }),
                );
            }
            base.insert(
                "tool_call_id".into(),
                json!(node.tool_call_id.as_deref().unwrap_or("")),
            );
            base.insert(
                "metadata".into(),
                json!({
                    "num_tokens": null,
                    "is_user_input": null,
                    "request_id": node.request_id,
                    "metrics": metrics_blob(node),
                    "finish_reason": null,
                    "extensions": extensions,
                    "created_at": to_iso(node.created_at),
                    "telemetry": { "source": "tool_result", "operation": tool_name },
                }),
            );
            Value::Object(base)
        }
    }
}

// ---------------------------------------------------------------------------
// parse_chat_message — devin blob → IR node (read path)
// ---------------------------------------------------------------------------

fn locations_from_acp(raw: &Value) -> Vec<ToolCallLocation> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let loc = obj(item)?;
            Some(ToolCallLocation {
                path: str_field(loc, "path")?.to_string(),
                line: num_field(loc, "line").map(|l| l as i64),
            })
        })
        .collect()
}

fn diffs_from_acp_content(raw: &Value) -> Vec<ToolCallDiff> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let c = obj(item)?;
            if c.get("type").and_then(Value::as_str) != Some("diff") {
                return None;
            }
            Some(ToolCallDiff {
                path: str_field(c, "path")?.to_string(),
                old_text: str_field(c, "oldText").map(str::to_string),
                new_text: str_field(c, "newText").map(str::to_string),
            })
        })
        .collect()
}

fn diffs_from_entry(raw: &Value) -> Vec<ToolCallDiff> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let c = obj(item)?;
            Some(ToolCallDiff {
                path: str_field(c, "path")?.to_string(),
                old_text: str_field(c, "oldText").map(str::to_string),
                new_text: str_field(c, "newText").map(str::to_string),
            })
        })
        .collect()
}

struct ToolCallExt {
    status: Option<ToolCallStatus>,
    locations: Vec<ToolCallLocation>,
    diffs: Vec<ToolCallDiff>,
}

/// Per-call snapshots from `chisel/tool_call_content` — the ACP `ToolCall`
/// recorded when the call was issued.
fn tool_call_ext_map(extensions: &Value) -> HashMap<String, ToolCallExt> {
    let mut map = HashMap::new();
    let Some(content) = obj(&extensions["chisel/tool_call_content"]) else {
        return map;
    };
    for (id, entry) in content {
        let Some(e) = obj(entry) else { continue };
        let status = from_acp_tool_call_status(str_field(e, "status"));
        let locations = locations_from_acp(field(e, "locations"));
        let diffs = diffs_from_acp_content(field(e, "content"));
        map.insert(
            id.clone(),
            ToolCallExt {
                status,
                locations,
                diffs,
            },
        );
    }
    map
}

fn parse_tool_calls(raw: &Value, ext_by_id: &HashMap<String, ToolCallExt>) -> Vec<ToolCall> {
    let Value::Array(items) = raw else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|tc| {
            let entry = obj(tc)?;
            let id = str_field(entry, "id").unwrap_or("").to_string();
            let ext = ext_by_id.get(&id);
            // Sepia-written calls keep locations/diffs on the call itself;
            // the chisel extension is the store-native carrier and wins.
            let locations = ext.filter(|e| !e.locations.is_empty()).map_or_else(
                || locations_from_acp(field(entry, "locations")),
                |e| e.locations.clone(),
            );
            let diffs = ext.filter(|e| !e.diffs.is_empty()).map_or_else(
                || diffs_from_entry(field(entry, "diffs")),
                |e| e.diffs.clone(),
            );
            Some(ToolCall {
                id,
                name: str_field(entry, "name").unwrap_or("unknown").to_string(),
                arguments: entry.get("arguments").cloned().unwrap_or(json!({})),
                index: num_field(entry, "index").unwrap_or(0.0) as i64,
                kind: str_field(entry, "kind").unwrap_or("function").to_string(),
                status: ext.and_then(|e| e.status),
                exit_code: None,
                duration_ms: None,
                locations,
                diffs,
            })
        })
        .collect()
}

/// Outcome fields a `role: "tool"` chat message keeps in its chisel
/// extensions.
fn tool_result_from_extensions(extensions: &Value) -> Option<ToolResultInfo> {
    let ext = obj(extensions)?;
    let success = field(ext, "chisel/tool_result_meta")
        .get("success")
        .and_then(Value::as_bool);
    let exit_code = field(ext, "chisel/terminal_output")
        .get("exit")
        .and_then(Value::as_object)
        .and_then(|o| num_field(o, "exit_code"));
    let duration_ms = field(ext, "chisel/tool_call_timing")
        .as_object()
        .and_then(|o| num_field(o, "duration_ms"));
    if success.is_none() && exit_code.is_none() && duration_ms.is_none() {
        return None;
    }
    Some(ToolResultInfo {
        status: if success == Some(false) {
            ToolCallStatus::Error
        } else {
            ToolCallStatus::Success
        },
        exit_code: exit_code.map(|c| c as i64),
        duration_ms,
    })
}

pub fn parse_chat_message(
    chat_message: &Value,
    row_metadata: &Value,
    node_id: i64,
    parent_node_id: Option<i64>,
    created_at: f64,
) -> MessageNode {
    let empty = Map::new();
    let msg = chat_message.as_object().unwrap_or(&empty);
    let role = match str_field(msg, "role") {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        Some("tool") => Role::Tool,
        _ => Role::System,
    };
    let content = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        other => serde_json::to_string(&other.cloned().unwrap_or_default()).unwrap_or_default(),
    };
    let meta = msg.get("metadata").and_then(Value::as_object);
    let extensions = meta
        .and_then(|m| m.get("extensions"))
        .cloned()
        .unwrap_or(Value::Null);
    let blocks = blocks_from_acp(&extensions["chisel/acp-content-blocks"]);
    let tool_calls = parse_tool_calls(field(msg, "tool_calls"), &tool_call_ext_map(&extensions));
    let thinking = field(msg, "thinking")["thinking"]
        .as_str()
        .map(str::to_string);
    let thinking_signature = field(msg, "thinking")["signature"]
        .as_str()
        .map(str::to_string);
    let tool_call_id = field(msg, "tool_call_id").as_str().map(str::to_string);
    let tool_name = extensions["chisel/tool_result_meta"]["kind"]
        .as_str()
        .map(str::to_string);
    let usage = meta
        .and_then(|m| m.get("metrics"))
        .and_then(usage_from_metrics);
    let model = meta
        .and_then(|m| m.get("generation_model"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let request_id = meta
        .and_then(|m| m.get("request_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let finish_reason = meta
        .and_then(|m| m.get("finish_reason"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let tool_result = if role == Role::Tool {
        tool_result_from_extensions(&extensions)
    } else {
        None
    };

    MessageNode {
        node_id,
        parent_node_id,
        role,
        content,
        blocks,
        tool_calls,
        tool_call_id,
        tool_name,
        thinking,
        thinking_signature,
        usage,
        model,
        request_id,
        finish_reason,
        tool_result,
        created_at,
        metadata: row_metadata.clone(),
    }
}

// ---------------------------------------------------------------------------
// Session row assembly
// ---------------------------------------------------------------------------

pub fn parse_json_or(s: Option<&str>, fallback: Value) -> Value {
    s.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(fallback)
}

#[derive(Clone, Debug, Default)]
pub struct SessionRow {
    pub id: String,
    pub working_directory: String,
    pub backend_type: String,
    pub model: String,
    pub agent_mode: String,
    pub created_at: f64,
    pub last_activity_at: f64,
    pub title: Option<String>,
    pub main_chain_id: Option<i64>,
    pub shell_last_seen_index: Option<i64>,
    pub cogs_json: Option<String>,
    pub workspace_dirs: Option<String>,
    pub hidden: i64,
    pub metadata: Option<String>,
}

pub fn session_from_devin_row(
    row: &SessionRow,
    nodes: Vec<MessageNode>,
    prompt_history: Vec<PromptHistoryEntry>,
    parent_session_id: Option<String>,
    agent_id: Option<String>,
) -> Session {
    let metadata = parse_json_or(row.metadata.as_deref(), json!({}));
    Session {
        id: row.id.clone(),
        title: row.title.clone().unwrap_or_else(|| row.id.clone()),
        working_directory: row.working_directory.clone(),
        backend_type: row.backend_type.clone(),
        agent_mode: row.agent_mode.clone(),
        model: row.model.clone(),
        created_at: row.created_at,
        last_activity_at: row.last_activity_at,
        main_chain_id: row.main_chain_id.unwrap_or(0),
        shell_last_seen_index: row.shell_last_seen_index.unwrap_or(0),
        cogs_json: row.cogs_json.clone().unwrap_or_else(|| "[]".into()),
        workspace_dirs: row.workspace_dirs.clone().unwrap_or_else(|| "[]".into()),
        hidden: row.hidden,
        parent_session_id,
        agent_id,
        checkpoints: shared::checkpoints_from_metadata(&metadata),
        metadata,
        nodes,
        prompt_history,
    }
}

pub use shared::{SESSION_CHECKPOINTS_KEY as CHECKPOINTS_KEY, tool_node_outcomes};
