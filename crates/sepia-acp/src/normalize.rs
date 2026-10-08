//! Tolerant ACP normalization — real agents emit malformed/partial
//! updates; every field degrades to a default instead of failing the
//! stream. Port of `packages/acp/src/normalize.ts`.

use std::sync::atomic::{AtomicU64, Ordering};

use sepia_core::{ToolCallDiff, ToolCallLocation};
use serde_json::{Map, Value};

use crate::types::{
    AcpSessionUpdate, AvailableCommand, PermissionOption, PermissionRequest, PlanEntry,
    ToolCallContent,
};

static NULL: Value = Value::Null;

pub fn as_record(value: &Value) -> &Map<String, Value> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
    value.as_object().unwrap_or(&EMPTY)
}

pub fn as_array(value: &Value) -> &Vec<Value> {
    static EMPTY: Vec<Value> = Vec::new();
    value.as_array().unwrap_or(&EMPTY)
}

pub fn as_string(value: &Value) -> Option<&str> {
    value.as_str()
}

/// Numbers arrive as JSON numbers, occasionally as numeric strings.
pub fn as_number_or_null(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return n.is_finite().then_some(n);
    }
    value
        .as_str()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

pub fn field<'a>(obj: &'a Map<String, Value>, key: &str) -> &'a Value {
    obj.get(key).unwrap_or(&NULL)
}

fn text_of(content: &Value) -> String {
    as_string(field(as_record(content), "text"))
        .unwrap_or_default()
        .to_string()
}

fn location_of(value: &Value) -> ToolCallLocation {
    let loc = as_record(value);
    ToolCallLocation {
        path: as_string(field(loc, "path"))
            .unwrap_or_default()
            .to_string(),
        line: as_number_or_null(field(loc, "line")).map(|l| l as i64),
    }
}

/// `diff` entries of a tool call's `content`.
fn diff_of(value: &Value) -> Option<ToolCallDiff> {
    let c = as_record(value);
    if as_string(field(c, "type")) != Some("diff") {
        return None;
    }
    Some(ToolCallDiff {
        path: as_string(field(c, "path"))?.to_string(),
        old_text: as_string(field(c, "oldText")).map(str::to_string),
        new_text: as_string(field(c, "newText")).map(str::to_string),
    })
}

fn diffs_of(content: &Value) -> Vec<ToolCallDiff> {
    as_array(content).iter().filter_map(diff_of).collect()
}

/// Non-diff `content` entries — `terminal` refs and wrapped
/// `ContentBlock`s (text + image kept, the rest of the union dropped).
fn tool_call_content_of(value: &Value) -> Option<ToolCallContent> {
    let c = as_record(value);
    match as_string(field(c, "type"))? {
        "terminal" => {
            let terminal_id =
                as_string(field(c, "terminalId")).or_else(|| as_string(field(c, "id")))?;
            let output = as_string(field(c, "output"))
                .or_else(|| as_string(field(c, "data")))
                .or_else(|| as_string(field(c, "text")))
                .map(str::to_string);
            Some(ToolCallContent::Terminal {
                terminal_id: terminal_id.to_string(),
                output,
            })
        }
        "content" => {
            let block = as_record(field(c, "content"));
            match as_string(field(block, "type"))? {
                "text" => Some(ToolCallContent::Text {
                    text: as_string(field(block, "text"))?.to_string(),
                }),
                "image" => {
                    let data = as_string(field(block, "data")).map(str::to_string);
                    let uri = as_string(field(block, "uri")).map(str::to_string);
                    if data.is_none() && uri.is_none() {
                        return None;
                    }
                    Some(ToolCallContent::Image {
                        data,
                        uri,
                        mime_type: as_string(field(block, "mimeType")).map(str::to_string),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn contents_of(content: &Value) -> Vec<ToolCallContent> {
    as_array(content)
        .iter()
        .filter_map(tool_call_content_of)
        .collect()
}

fn option_of(value: &Value) -> PermissionOption {
    let o = as_record(value);
    PermissionOption {
        option_id: as_string(field(o, "optionId"))
            .unwrap_or_default()
            .to_string(),
        name: as_string(field(o, "name")).unwrap_or_default().to_string(),
        kind: as_string(field(o, "kind")).unwrap_or_default().to_string(),
    }
}

/// Map a raw `session/update` params payload to the normalized union.
/// Unknown `sessionUpdate` kinds land on `Other` with the raw payload.
pub fn normalize_update(update: &Value) -> AcpSessionUpdate {
    let u = as_record(update);
    match as_string(field(u, "sessionUpdate")) {
        Some("user_message_chunk") => AcpSessionUpdate::UserMessageChunk {
            text: text_of(field(u, "content")),
        },
        Some("agent_message_chunk") => AcpSessionUpdate::AgentMessageChunk {
            text: text_of(field(u, "content")),
        },
        Some("agent_thought_chunk") => AcpSessionUpdate::AgentThoughtChunk {
            text: text_of(field(u, "content")),
        },
        Some("tool_call") => {
            let contents = contents_of(field(u, "content"));
            AcpSessionUpdate::ToolCall {
                tool_call_id: as_string(field(u, "toolCallId"))
                    .unwrap_or_default()
                    .to_string(),
                title: as_string(field(u, "title")).unwrap_or_default().to_string(),
                status: as_string(field(u, "status"))
                    .unwrap_or_default()
                    .to_string(),
                tool_kind: as_string(field(u, "kind")).unwrap_or_default().to_string(),
                raw_input: field(u, "rawInput").clone(),
                locations: as_array(field(u, "locations"))
                    .iter()
                    .map(location_of)
                    .collect(),
                diffs: diffs_of(field(u, "content")),
                contents: (!contents.is_empty()).then_some(contents),
            }
        }
        Some("tool_call_update") => {
            let locations: Vec<ToolCallLocation> = as_array(field(u, "locations"))
                .iter()
                .map(location_of)
                .collect();
            let diffs = diffs_of(field(u, "content"));
            let contents = contents_of(field(u, "content"));
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: as_string(field(u, "toolCallId"))
                    .unwrap_or_default()
                    .to_string(),
                status: as_string(field(u, "status"))
                    .unwrap_or_default()
                    .to_string(),
                title: as_string(field(u, "title")).map(str::to_string),
                raw_input: u.get("rawInput").cloned(),
                raw_output: u.get("rawOutput").cloned(),
                locations: (!locations.is_empty()).then_some(locations),
                diffs: (!diffs.is_empty()).then_some(diffs),
                contents: (!contents.is_empty()).then_some(contents),
            }
        }
        Some("plan") => AcpSessionUpdate::Plan {
            entries: as_array(field(u, "entries"))
                .iter()
                .map(|entry| {
                    let e = as_record(entry);
                    PlanEntry {
                        content: as_string(field(e, "content"))
                            .unwrap_or_default()
                            .to_string(),
                        status: as_string(field(e, "status"))
                            .unwrap_or_default()
                            .to_string(),
                    }
                })
                .collect(),
        },
        Some("current_mode_update") => AcpSessionUpdate::CurrentModeUpdate {
            mode_id: as_string(field(u, "currentModeId"))
                .unwrap_or_default()
                .to_string(),
        },
        Some("available_commands_update") => AcpSessionUpdate::AvailableCommandsUpdate {
            commands: as_array(field(u, "availableCommands"))
                .iter()
                .map(|command| {
                    let c = as_record(command);
                    AvailableCommand {
                        name: as_string(field(c, "name")).unwrap_or_default().to_string(),
                        description: as_string(field(c, "description")).map(str::to_string),
                    }
                })
                .collect(),
        },
        other => AcpSessionUpdate::Other {
            session_update: other.unwrap_or_default().to_string(),
            raw: update.clone(),
        },
    }
}

static PERMISSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Normalize a `session/request_permission` params payload and mint the
/// `requestId` the out-of-band response will settle by.
pub fn normalize_permission(params: &Value) -> PermissionRequest {
    let p = as_record(params);
    let tool_call = as_record(field(p, "toolCall"));
    let session_id = as_string(field(p, "sessionId"))
        .unwrap_or_default()
        .to_string();
    let tool_call_id = as_string(field(tool_call, "toolCallId")).map(str::to_string);
    let counter = PERMISSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    PermissionRequest {
        request_id: format!(
            "{session_id}:{}:{counter}",
            tool_call_id.as_deref().unwrap_or("none")
        ),
        session_id,
        tool_call_id,
        title: as_string(field(tool_call, "title"))
            .unwrap_or_default()
            .to_string(),
        options: as_array(field(p, "options"))
            .iter()
            .map(option_of)
            .collect(),
    }
}
