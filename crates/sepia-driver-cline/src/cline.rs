//! `Cline.ts` port — Cline session-dir parsing → session IR, plus the
//! manifest/transcript writers an export needs.
//!
//! A Cline session lives in `<dataDir>/sessions/<session-id>/` as a pair:
//! `<id>.json` (manifest) and `<id>.messages.json` (the provider-shaped
//! message log). Reading is deliberately tolerant: malformed entries
//! degrade instead of failing the session.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sepia_core::domain::{
    Block, CheckpointRef, ConversionError, MessageNode, PromptHistoryEntry, REDACTED_THINKING,
    Role, Session, TokenUsage, ToolCall, ToolCallDiff, ToolCallLocation, ToolCallStatus,
    ToolResultInfo,
};
use sepia_core::shared::{self, ToolCallOutcome};
use serde_json::{Map, Value, json};

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

static NULL: Value = Value::Null;

/// `v.get(key)` tolerating a non-object input the way `obj[key]` does.
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

fn finite_number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch as u32 >= 32 || ch == '\t' || ch == '\n' || ch == '\r' {
            out.push(ch);
        }
    }
    out
}

fn clean_user_text(text: &str) -> String {
    if text.starts_with("<user_input") && text.contains("</user_input>") {
        let start = text.find('>').map_or(0, |i| i + 1);
        let end = text.rfind("</user_input>").unwrap_or(text.len());
        return text[start..end].trim().to_string();
    }
    text.to_string()
}

/// `String(x)` semantics for the values `keyOf`/`asText` stringify — the
/// object form (`[object Object]`) included, since `keyOf(input)` calls
/// land on whole argument objects.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// JS truthiness — used where the TS guards on a bare value (`c.text`).
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn key_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(js_string).collect();
            if parts.len() == 1 {
                parts[0].clone()
            } else {
                parts.join("\n")
            }
        }
        other => js_string(other),
    }
}

/// A list field in a Cline log is not always a list: the call is replayed as the
/// provider wrote it, so `commands`, `files`, `queries` and `requests` can arrive
/// as a JSON string (`"[\"a\", \"b\"]"`) or as a bare string when the model sent
/// one. Either way the items are still the ones to run.
fn as_list(value: &Value) -> Vec<Value> {
    if let Value::Array(items) = value {
        return items.clone();
    }
    let Value::String(s) = value else {
        return Vec::new();
    };
    let text = s.trim();
    if text.starts_with('[') {
        if let Ok(Value::Array(parsed)) = serde_json::from_str::<Value>(text) {
            return parsed;
        }
    }
    if text.is_empty() {
        Vec::new()
    } else {
        vec![json!(text)]
    }
}

fn make_tool_call_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("chatcmpl-tool-{}", &raw[..16])
}

/// One entry of a Cline message `content` array mapped onto the IR block
/// union. `tool_use`/`tool_result`/`thinking` entries have their own IR
/// fields and return `None` here; `image`/`document` are the provider's
/// attachment forms — `source` carries `base64`/`url`/`text` variants.
fn block_from_cline(item: &Value) -> Option<Block> {
    let c = item.as_object()?;
    let source = c.get("source").and_then(Value::as_object);
    let source_field = |key: &str| source.and_then(|s| s.get(key)).and_then(Value::as_str);
    match c.get("type").and_then(Value::as_str) {
        Some("text") => Some(Block::Text {
            text: str_field(c, "text")?.to_string(),
        }),
        Some("image") => {
            let data = str_field(c, "data").or_else(|| source_field("data"));
            let uri = str_field(c, "url").or_else(|| source_field("url"));
            if data.is_none() && uri.is_none() {
                return None;
            }
            let mime_type = source_field("media_type")
                .or_else(|| str_field(c, "media_type"))
                .or_else(|| str_field(c, "mimeType"));
            Some(Block::Image {
                data: data.map(str::to_string),
                mime_type: mime_type.map(str::to_string),
                uri: uri.map(str::to_string),
            })
        }
        Some("document") => {
            let text = source_field("text").or_else(|| str_field(c, "text"));
            let data = source_field("data").or_else(|| str_field(c, "data"));
            let uri = source_field("url").or_else(|| str_field(c, "url"));
            if text.is_none() && data.is_none() && uri.is_none() {
                return None;
            }
            let mime_type = source_field("media_type")
                .or_else(|| str_field(c, "media_type"))
                .or_else(|| str_field(c, "mimeType"));
            Some(Block::File {
                uri: uri.map(str::to_string),
                name: str_field(c, "title").map(str::to_string),
                mime_type: mime_type.map(str::to_string),
                size: None,
                text: text.map(str::to_string),
                data: data.map(str::to_string),
            })
        }
        _ => None,
    }
}

/// The block list a Cline user message's `content` array carries. Kept only
/// when a non-text block is present — an all-text list duplicates `content`.
fn blocks_from_cline_content(content: &Value) -> Vec<Block> {
    let Value::Array(items) = content else {
        return Vec::new();
    };
    let blocks: Vec<Block> = items.iter().filter_map(block_from_cline).collect();
    if blocks.iter().any(|b| !matches!(b, Block::Text { .. })) {
        blocks
    } else {
        Vec::new()
    }
}

/// An IR block back into the Cline log's provider-shaped content form. Text
/// blocks are skipped — the node's `content` already writes one — while
/// file/audio blocks degrade to a text mention, the only honest form the log
/// format has for an attachment reference.
fn to_cline_content_block(block: &Block) -> Option<Value> {
    match block {
        Block::Text { .. } => None,
        Block::Image {
            data,
            mime_type,
            uri,
        } => {
            if let Some(data) = data {
                let mut source = Map::new();
                source.insert("type".into(), json!("base64"));
                if let Some(mime) = mime_type {
                    source.insert("media_type".into(), json!(mime));
                }
                source.insert("data".into(), json!(data));
                return Some(json!({ "type": "image", "source": source }));
            }
            if let Some(uri) = uri {
                return Some(json!({ "type": "image", "source": { "type": "url", "url": uri } }));
            }
            Some(json!({ "type": "text", "text": "[image]" }))
        }
        Block::Audio { mime_type, .. } => Some(json!({
            "type": "text",
            "text": format!("[audio: {}]", mime_type.as_deref().unwrap_or("attachment")),
        })),
        Block::File {
            uri, name, text, ..
        } => {
            let label = format!(
                "[file: {}]",
                name.as_deref().or(uri.as_deref()).unwrap_or("attachment")
            );
            let text_out = match text {
                None => label,
                Some(t) => format!("{label}\n{t}"),
            };
            Some(json!({ "type": "text", "text": text_out }))
        }
    }
}

/// The lineage a sub-agent session id embeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentInfo {
    pub parent_session_id: String,
    pub agent_id: String,
}

/// Sub-agent session ids embed their lineage: `<parent>__teamtask__<agent>__<rand>`
/// for team tasks and `<parent>__agent_<agent>` for spawned agents — the index
/// row's `parent_session_id`/`agent_id` repeat exactly these segments, so the
/// manifest-only read paths can recover them without opening `db/sessions.db`.
pub fn cline_subagent_info(session_id: &str) -> Option<SubagentInfo> {
    // Nested team tasks chain the markers, and the parent is the session one
    // level up — so the split happens at the last `__teamtask__`, not the first.
    if let Some(teamtask) = session_id.rfind("__teamtask__") {
        let rest = &session_id[teamtask + "__teamtask__".len()..];
        let agent_id = rest.rfind("__").map_or(rest, |sep| &rest[..sep]);
        if agent_id.is_empty() {
            return None;
        }
        return Some(SubagentInfo {
            parent_session_id: session_id[..teamtask].to_string(),
            agent_id: agent_id.to_string(),
        });
    }
    if let Some(spawned) = session_id.find("__agent_") {
        return Some(SubagentInfo {
            parent_session_id: session_id[..spawned].to_string(),
            agent_id: session_id[spawned + 2..].to_string(),
        });
    }
    None
}

/// The first field that carries the items of a call. Tool inputs are not uniform
/// across the sessions Cline has written: commands arrive as `commands` or as a
/// single `command`, reads as `files` or as `path` (+ line range).
fn first_list(input: &Value, keys: &[&str]) -> Vec<Value> {
    for key in keys {
        let items = as_list(field(input, key));
        if !items.is_empty() {
            return items;
        }
    }
    Vec::new()
}

/// A list item is usually the text itself; a wrapper object still carries it.
fn as_text(value: &Value, keys: &[&str]) -> String {
    if let Value::String(s) = value {
        return s.clone();
    }
    if let Value::Object(obj) = value {
        for key in keys {
            if let Some(inner) = obj.get(*key).and_then(Value::as_str) {
                return inner.to_string();
            }
        }
    }
    match value {
        Value::Null => String::new(),
        v => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// One mapped call. `locations`/`diffs` carry the file refs the tool input
/// recorded — reads name the files they touch, `editor` inputs are already
/// a `{path, old_text, new_text}` diff.
struct MappedCall {
    name: String,
    arguments: Value,
    result_key: String,
    locations: Option<Vec<ToolCallLocation>>,
    diffs: Option<Vec<ToolCallDiff>>,
}

fn map_tool_use(cline_tool: &Value) -> Vec<MappedCall> {
    let name = cline_tool
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let input = match cline_tool.get("input") {
        None | Some(Value::Null) => json!({}),
        Some(v) => v.clone(),
    };
    let mut calls: Vec<MappedCall> = Vec::new();
    // A call whose items cannot be read is kept as it arrived: dropping it would
    // leave its result looking like output nobody asked for.
    macro_rules! raw {
        () => {
            calls.push(MappedCall {
                name: name.clone(),
                arguments: input.clone(),
                result_key: key_of(&input),
                locations: None,
                diffs: None,
            })
        };
    }

    match name.as_str() {
        "read_files" => {
            let files = first_list(&input, &["files", "paths", "path", "file_path"]);
            if files.is_empty() {
                raw!();
            } else {
                for f in &files {
                    let path = as_text(f, &["path", "file_path"]);
                    calls.push(MappedCall {
                        name: "read".into(),
                        arguments: json!({ "file_path": path }),
                        result_key: path.clone(),
                        locations: if path.is_empty() {
                            None
                        } else {
                            Some(vec![ToolCallLocation { path, line: None }])
                        },
                        diffs: None,
                    });
                }
            }
        }
        "run_commands" => {
            let commands = first_list(&input, &["commands", "command", "cmd"]);
            if commands.is_empty() {
                raw!();
            } else {
                for cmd in &commands {
                    let command = as_text(cmd, &["command"]);
                    calls.push(MappedCall {
                        name: "exec".into(),
                        arguments: json!({ "command": command }),
                        result_key: command,
                        locations: None,
                        diffs: None,
                    });
                }
            }
        }
        "search_codebase" => {
            let queries = first_list(&input, &["queries", "query", "pattern"]);
            if queries.is_empty() {
                raw!();
            } else {
                for q in &queries {
                    let patterns: &[Value] = match q {
                        Value::Array(inner) => inner.as_slice(),
                        other => std::slice::from_ref(other),
                    };
                    for pattern in patterns {
                        let text = as_text(pattern, &["pattern", "query"]);
                        calls.push(MappedCall {
                            name: "grep".into(),
                            arguments: json!({ "pattern": text }),
                            result_key: text,
                            locations: None,
                            diffs: None,
                        });
                    }
                }
            }
        }
        "fetch_web_content" => {
            let requests = first_list(&input, &["requests", "url"]);
            if requests.is_empty() {
                raw!();
            } else {
                for req in &requests {
                    let url = as_text(req, &["url"]);
                    calls.push(MappedCall {
                        name: "webfetch".into(),
                        arguments: json!({ "url": url }),
                        result_key: url,
                        locations: None,
                        diffs: None,
                    });
                }
            }
        }
        "editor" => {
            let path = as_text(field(&input, "path"), &["path"]);
            let old_text = field(&input, "old_text");
            let new_text = field(&input, "new_text");
            if path.is_empty() {
                raw!();
            } else {
                let locations = vec![ToolCallLocation {
                    path: path.clone(),
                    line: None,
                }];
                let create =
                    old_text.is_null() || matches!(old_text, Value::String(s) if s == "null");
                if create {
                    calls.push(MappedCall {
                        name: "write".into(),
                        arguments: json!({ "file_path": path, "content": new_text }),
                        result_key: path.clone(),
                        locations: Some(locations),
                        // A create has no `old_text` — a newText-only diff marks it so.
                        diffs: Some(vec![ToolCallDiff {
                            path: path.clone(),
                            old_text: None,
                            new_text: new_text.as_str().map(str::to_string),
                        }]),
                    });
                } else {
                    calls.push(MappedCall {
                        name: "edit".into(),
                        arguments: json!({
                            "file_path": path,
                            "old_string": old_text,
                            "new_string": new_text,
                        }),
                        result_key: path.clone(),
                        locations: Some(locations),
                        diffs: Some(vec![ToolCallDiff {
                            path: path.clone(),
                            old_text: Some(match old_text {
                                Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            }),
                            new_text: new_text.as_str().map(str::to_string),
                        }]),
                    });
                }
            }
        }
        _ => {
            raw!();
        }
    }

    calls
}

/// One devin call's share of a Cline tool result: the text plus its success flag.
struct ResultShare {
    content: Option<String>,
    success: Option<bool>,
}

/// One pending devin call waiting for its Cline result entry.
#[derive(Clone)]
struct PendingCall {
    devin: ToolCall,
    result_key: String,
}

/**
 * The value each call's result should carry, aligned with `devinCalls`.
 *
 * Cline answers a multi-item call with one entry per item, in the order the
 * items were requested, but an entry's `query` is not always the text its call
 * carried: a read of a line range comes back as `path:start-end`, an array query
 * comes back joined, and a command can come back with different escaping. A key
 * match is therefore only a hint — entries that match no call fill the calls
 * that matched no entry, in order — so no result is silently dropped.
 */
fn tool_result_shares(cline_result: &Value, devin_calls: &[PendingCall]) -> Vec<ResultShare> {
    let content = field(cline_result, "content");

    if let Value::String(s) = content {
        return devin_calls
            .iter()
            .map(|_| ResultShare {
                content: Some(s.clone()),
                success: None,
            })
            .collect();
    }
    let Value::Array(items) = content else {
        let share = if content.is_null() {
            None
        } else {
            Some(serde_json::to_string(content).unwrap_or_default())
        };
        return devin_calls
            .iter()
            .map(|_| ResultShare {
                content: share.clone(),
                success: None,
            })
            .collect();
    };

    let entries: Vec<&Value> = items.iter().filter(|item| !item.is_null()).collect();
    let values: Vec<String> = entries
        .iter()
        .map(|item| {
            if let Value::String(s) = item {
                return s.clone();
            }
            match item.get("result") {
                Some(Value::String(r)) => r.clone(),
                Some(v) if !v.is_null() => serde_json::to_string(v).unwrap_or_default(),
                _ => serde_json::to_string(item).unwrap_or_default(),
            }
        })
        .collect();
    let successes: Vec<Option<bool>> = entries
        .iter()
        .map(|item| {
            if item.is_object() {
                item.get("success").and_then(Value::as_bool)
            } else {
                None
            }
        })
        .collect();
    let keys: Vec<String> = entries
        .iter()
        .map(|item| {
            if item.is_object() {
                let keyed = item
                    .get("query")
                    .filter(|v| !v.is_null())
                    .or_else(|| item.get("url").filter(|v| !v.is_null()));
                keyed.map_or_else(String::new, key_of)
            } else {
                String::new()
            }
        })
        .collect();

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut assigned: Vec<Option<usize>> = Vec::with_capacity(devin_calls.len());
    for call in devin_calls {
        let index = keys
            .iter()
            .enumerate()
            .find(|(i, key)| *key == &call.result_key && !claimed.contains(i))
            .map(|(i, _)| i);
        if let Some(i) = index {
            claimed.insert(i);
        }
        assigned.push(index);
    }

    let unclaimed: Vec<usize> = (0..entries.len())
        .filter(|i| !claimed.contains(i))
        .collect();
    let mut next = 0;
    for slot in &mut assigned {
        if next >= unclaimed.len() {
            break;
        }
        if slot.is_none() {
            *slot = Some(unclaimed[next]);
            next += 1;
        }
    }

    let mut out: Vec<ResultShare> = assigned
        .iter()
        .map(|index| match index {
            None => ResultShare {
                content: None,
                success: None,
            },
            Some(i) => ResultShare {
                content: Some(values[*i].clone()),
                success: successes[*i],
            },
        })
        .collect();

    // Nothing a result carried is thrown away: entries no call could hold — a
    // tool that could not be split into calls, an answer with more parts than
    // calls — ride along on the last call, and a lone call keeps the whole answer.
    let tail_indices = &unclaimed[next..];
    let tail = tail_indices
        .iter()
        .map(|i| values[*i].clone())
        .collect::<Vec<_>>()
        .join("\n");
    if !tail.is_empty() {
        if let Some(last) = out.last_mut() {
            let failed = last.success == Some(false)
                || tail_indices.iter().any(|i| successes[*i] == Some(false));
            let merged_success = last.success;
            last.content = Some(match last.content.take() {
                None => tail,
                Some(c) => format!("{c}\n{tail}"),
            });
            last.success = if merged_success.is_none() && !failed {
                None
            } else if failed {
                Some(false)
            } else {
                Some(true)
            };
        }
    }

    out
}

fn build_system_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    content: &str,
    created_at: f64,
    is_prefix: bool,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role: Role::System,
        content: sanitize(content),
        blocks: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at,
        metadata: json!({
            "summarized_from": null,
            "num_tokens_preceding": null,
            "is_system_prefix": is_prefix,
        }),
    }
}

fn build_user_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    text: &str,
    created_at: f64,
    blocks: Vec<Block>,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role: Role::User,
        content: sanitize(text),
        blocks,
        tool_calls: Vec::new(),
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at,
        metadata: Value::Null,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_assistant_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    text: &str,
    thinking: &str,
    thinking_signature: Option<String>,
    tool_calls: Vec<ToolCall>,
    created_at: f64,
    rendered: bool,
    usage: Option<TokenUsage>,
    model: Option<String>,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role: Role::Assistant,
        content: sanitize(text),
        blocks: Vec::new(),
        tool_calls,
        tool_call_id: None,
        tool_name: None,
        thinking: if thinking.is_empty() {
            None
        } else {
            Some(sanitize(thinking))
        },
        thinking_signature,
        usage,
        model,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at,
        metadata: if rendered {
            json!({
                "summarized_from": null,
                "num_tokens_preceding": null,
                "is_system_prefix": null,
            })
        } else {
            Value::Null
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tool_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    tool_call_id: &str,
    content: &str,
    tool_name: &str,
    tool_arguments: &Value,
    created_at: f64,
    tool_result: Option<ToolResultInfo>,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role: Role::Tool,
        content: sanitize(content),
        blocks: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: Some(tool_call_id.to_string()),
        tool_name: Some(tool_name.to_string()),
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result,
        created_at,
        metadata: if tool_arguments.is_null() {
            Value::Null
        } else {
            json!({ "toolArguments": tool_arguments })
        },
    }
}

fn push_node(
    nodes: &mut Vec<MessageNode>,
    parent: Option<i64>,
    build: impl FnOnce(i64, Option<i64>) -> MessageNode,
) -> i64 {
    let node = build(i64::try_from(nodes.len()).unwrap_or(i64::MAX), parent);
    let id = node.node_id;
    nodes.push(node);
    id
}

/// Merge `clineMessageIndex` — the source entry's position in the
/// `messages` array — into a node's metadata. The in-place rewind writer
/// cuts the array on this key; nodes sharing one entry (the assistant
/// twins, a multi-result turn) carry the same index and survive together.
fn with_source_index(node: &MessageNode, index: i64) -> MessageNode {
    let mut next = node.clone();
    let mut meta = next.metadata.as_object().cloned().unwrap_or_default();
    meta.insert("clineMessageIndex".into(), json!(index));
    next.metadata = Value::Object(meta);
    next
}

fn ts_to_epoch_seconds(ts: &Value, fallback: f64) -> f64 {
    if let Some(n) = ts.as_f64() {
        return if n > 1e12 { (n / 1000.0).floor() } else { n };
    }
    fallback
}

/// Cline `metrics` uses camelCase token keys; `cost` may be absent.
fn usage_from_cline_metrics(metrics: &Value) -> Option<TokenUsage> {
    let m = metrics.as_object()?;
    let input = m.get("inputTokens").and_then(finite_number);
    let output = m.get("outputTokens").and_then(finite_number);
    if input.is_none() && output.is_none() {
        return None;
    }
    Some(TokenUsage {
        input: input.unwrap_or(0.0),
        output: output.unwrap_or(0.0),
        cache_read: m.get("cacheReadTokens").and_then(finite_number),
        cache_write: m.get("cacheWriteTokens").and_then(finite_number),
        thinking: None,
        cost: m.get("cost").and_then(finite_number),
    })
}

fn model_from_message(m: &Value) -> Option<String> {
    field(field(m, "modelInfo"), "id")
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// One `metadata.checkpoint` history entry: `{ref, createdAt (epoch ms),
/// runCount, kind}` — a shadow-git stash/commit sha in the workspace repo.
/// Malformed entries are dropped, not fatal.
fn checkpoint_entry(raw: &Value) -> Option<CheckpointRef> {
    let c = raw.as_object()?;
    let r#ref = str_field(c, "ref")?.to_string();
    let created_at = c.get("createdAt").and_then(finite_number)?;
    Some(CheckpointRef {
        r#ref,
        created_at,
        run_count: c.get("runCount").and_then(finite_number).map(|n| n as i64),
        kind: str_field(c, "kind").map(str::to_string),
    })
}

/// The manifest's `metadata.checkpoint` blob — `{latest, history}` of
/// shadow-git refs. `latest` normally duplicates the history tail; when it
/// doesn't (older manifests kept `latest` only), it is appended so the
/// newest ref is never lost.
pub fn checkpoints_from_manifest(meta: &Value) -> Vec<CheckpointRef> {
    let Some(blob) = field(meta, "metadata")
        .get("checkpoint")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut history: Vec<CheckpointRef> = blob
        .get("history")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items.iter().filter_map(checkpoint_entry).collect()
        });
    if let Some(latest) = checkpoint_entry(blob.get("latest").unwrap_or(&NULL)) {
        if history.iter().all(|entry| entry.r#ref != latest.r#ref) {
            history.push(latest);
        }
    }
    history
}

fn parse_messages_data(raw: &Value) -> Vec<Value> {
    if let Value::Object(obj) = raw {
        if let Some(Value::Array(messages)) = obj.get("messages") {
            return messages.clone();
        }
    }
    Vec::new()
}

/// Read a Cline session directory into the IR. `session_id` overrides the
/// id the manifest/basename resolution would pick.
///
/// # Errors
/// `ConversionError` on a missing dir/manifest, unreadable files, invalid
/// JSON, or a build failure — same failure surface as `Cline.fromDirectory`.
pub fn from_directory(dir: &Path, session_id: Option<&str>) -> Result<Session, ConversionError> {
    let fail = |message: String| ConversionError {
        message,
        cause: Value::Null,
    };
    if !dir.exists() {
        return Err(fail(format!(
            "Cline session directory not found: {}",
            dir.display()
        )));
    }

    let entries =
        std::fs::read_dir(dir).map_err(|e| fail(format!("Cline conversion failed: {e}")))?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| fail(format!("Cline conversion failed: {e}")))?;
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let meta_name = names.iter().find(|name| {
        name.ends_with(".json")
            && !name.ends_with(".messages.json")
            && !name.contains(".compaction.")
    });
    let Some(meta_name) = meta_name else {
        return Err(fail(format!(
            "No session metadata json found in {}",
            dir.display()
        )));
    };

    let base = meta_name.strip_suffix(".json").unwrap_or(meta_name);
    let meta_path = dir.join(meta_name);
    let messages_path = dir.join(format!("{base}.messages.json"));

    let meta_raw = std::fs::read_to_string(&meta_path)
        .map_err(|e| fail(format!("Cline conversion failed: {e}")))?;
    let meta: Value = serde_json::from_str(&meta_raw)
        .map_err(|e| fail(format!("Cline conversion failed: {e}")))?;

    // Sub-agent manifests point `messages_path` into the parent's directory;
    // the `<id>.messages.json` sibling only exists for root sessions.
    let resolved_messages_path = meta
        .get("messages_path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map_or(messages_path, PathBuf::from);
    let messages_raw = std::fs::read_to_string(&resolved_messages_path)
        .map_err(|e| fail(format!("Cline conversion failed: {e}")))?;
    let messages_json: Value = serde_json::from_str(&messages_raw)
        .map_err(|e| fail(format!("Cline conversion failed: {e}")))?;
    let messages = parse_messages_data(&messages_json);

    build_session(dir, &meta, &messages, session_id)
        .map_err(|e| fail(format!("Failed to build session: {e}")))
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
        .floor()
}

/// Epoch seconds for an ISO-8601 string the way `new Date(x).getTime()/1000`
/// reads it. Only RFC 3339 input is recognized — anything else returns
/// `None` where the TS yields `NaN` (serde_json cannot emit NaN, so the
/// callers substitute "now" instead of propagating an unserializable date).
pub fn iso_to_seconds(text: &str) -> Option<f64> {
    let parsed =
        time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()?;
    let nanos = parsed.unix_timestamp_nanos();
    let secs = nanos / 1_000_000_000;
    let secs_i64 = i64::try_from(secs).ok()?;
    Some(secs_i64 as f64)
}

/// `new Date(ts*1000).toISOString()` — always 3-digit millis + `Z`.
pub fn to_iso_ms(ts_secs: f64) -> String {
    if !ts_secs.is_finite() {
        return String::new();
    }
    let Ok(format) = time::format_description::parse_borrowed::<3>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    ) else {
        return String::new();
    };
    match time::OffsetDateTime::from_unix_timestamp_nanos((ts_secs * 1e9) as i128) {
        Ok(t) => t.format(&format).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

fn build_session(
    dir: &Path,
    meta: &Value,
    messages: &[Value],
    session_id: Option<&str>,
) -> Result<Session, String> {
    let dir_str = dir.to_string_lossy().to_string();
    let cwd = meta
        .get("cwd")
        .and_then(Value::as_str)
        .map_or(dir_str.clone(), str::to_string);
    let title = meta
        .get("metadata")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .or_else(|| meta.get("prompt").and_then(Value::as_str))
        .unwrap_or("Imported session")
        .to_string();
    // `meta.model ?? "glm-5-2"` then `.replace(...)` — a non-string model
    // throws in the TS too (the "Failed to build session" path).
    let raw_model = match meta.get("model") {
        None | Some(Value::Null) => "glm-5-2".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("meta.model.replace is not a function".into()),
    };
    let model = raw_model
        .strip_prefix("cline-pass/")
        .unwrap_or(&raw_model)
        .to_string();
    let started_at = meta
        .get("started_at")
        .and_then(Value::as_str)
        .and_then(iso_to_seconds)
        .unwrap_or_else(now_seconds);
    let created_at = started_at;
    let last_activity_at = meta
        .get("ended_at")
        .and_then(Value::as_str)
        .and_then(iso_to_seconds)
        .unwrap_or_else(now_seconds);

    let mut nodes: Vec<MessageNode> = Vec::new();

    let sys_info = format!(
        "<system_info>\n\
         The following information is automatically generated context about your current environment.\n\
         Current workspace directories:\n  {cwd} (cwd)\n\n\
         Platform: linux\n\
         </system_info>"
    );

    let n0 = push_node(&mut nodes, None, |nid, parent| {
        build_system_node(nid, parent, &sys_info, created_at, true)
    });
    let n1 = push_node(&mut nodes, Some(n0), |nid, parent| {
        build_system_node(
            nid,
            parent,
            "<rules type=\"always-on\"></rules>",
            created_at,
            false,
        )
    });

    // The first text-bearing user message seeds the tree — its content (and
    // attachment blocks) are lifted onto the seed node rather than emitted
    // a second time inside the message loop.
    let mut first_user_index: i64 = -1;
    let mut first_user_text = String::new();
    let mut first_user_blocks: Vec<Block> = Vec::new();
    for (index, m) in messages.iter().enumerate() {
        if m.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = m.get("content").and_then(Value::as_array) else {
            continue;
        };
        let mut found = false;
        for c in content {
            if c.get("type").and_then(Value::as_str) == Some("text")
                && c.get("text").is_some_and(js_truthy)
            {
                first_user_index = i64::try_from(index).unwrap_or(i64::MAX);
                first_user_text = match c.get("text") {
                    Some(Value::String(s)) => clean_user_text(s),
                    // A truthy non-string `text` crashes `cleanUserText` in the
                    // TS — surface the same build failure.
                    Some(_) => {
                        return Err("cleanUserText: text.startsWith is not a function".into());
                    }
                    None => String::new(),
                };
                first_user_blocks = blocks_from_cline_content(field(m, "content"));
                found = true;
                break;
            }
        }
        if found {
            break;
        }
    }

    let n_user = push_node(&mut nodes, Some(n1), |nid, parent| {
        build_user_node(nid, parent, &first_user_text, created_at, first_user_blocks)
    });
    if first_user_index != -1 {
        nodes[usize::try_from(n_user).unwrap_or(0)] = with_source_index(
            &nodes[usize::try_from(n_user).unwrap_or(0)],
            first_user_index,
        );
    }
    let n_skills = push_node(&mut nodes, Some(n_user), |nid, parent| {
        build_system_node(
            nid,
            parent,
            "<available_skills></available_skills>",
            created_at,
            false,
        )
    });

    let mut pending_tool_calls: HashMap<String, Vec<PendingCall>> = HashMap::new();
    let mut tool_call_outcomes: HashMap<String, ToolCallOutcome> = HashMap::new();
    let mut last_rendered_assistant_node = n_skills;
    let mut last_tool_result_node: Option<i64> = None;
    let mut first_user_seen = false;

    /// Tag the just-emitted node with its source `messages` position.
    macro_rules! tag_last {
        ($index:expr) => {
            if let Some(last) = nodes.last_mut() {
                let tagged = with_source_index(last, $index);
                *last = tagged;
            }
        };
    }

    for (m_index, m) in messages.iter().enumerate() {
        let m_index = i64::try_from(m_index).unwrap_or(i64::MAX);
        let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
        let ts = ts_to_epoch_seconds(field(m, "ts"), created_at);

        if role == "user" {
            let empty: Vec<Value> = Vec::new();
            let content = m.get("content").and_then(Value::as_array).unwrap_or(&empty);
            let has_text = content
                .iter()
                .any(|c| c.get("type").and_then(Value::as_str) == Some("text"));
            let has_tool_result = content
                .iter()
                .any(|c| c.get("type").and_then(Value::as_str) == Some("tool_result"));

            if has_text && !has_tool_result {
                let mut text = String::new();
                for c in content {
                    if c.get("type").and_then(Value::as_str) == Some("text")
                        && c.get("text").is_some_and(js_truthy)
                    {
                        text = match c.get("text") {
                            Some(Value::String(s)) => clean_user_text(s),
                            Some(_) => {
                                return Err(
                                    "cleanUserText: text.startsWith is not a function".into()
                                );
                            }
                            None => String::new(),
                        };
                        break;
                    }
                }

                if !text.is_empty() {
                    if !first_user_seen {
                        first_user_seen = true;
                        continue;
                    }
                    last_tool_result_node = None;
                    let parent = Some(last_rendered_assistant_node);
                    let blocks = blocks_from_cline_content(field(m, "content"));
                    push_node(&mut nodes, parent, |nid, p| {
                        build_user_node(nid, p, &text, ts, blocks)
                    });
                    tag_last!(m_index);
                    last_rendered_assistant_node = nodes[nodes.len() - 1].node_id;
                }
            }

            if has_tool_result {
                for c in content {
                    if c.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let cline_tu_id = match c.get("tool_use_id") {
                        None | Some(Value::Null) => String::new(),
                        Some(v) => js_string(v),
                    };
                    let devin_calls = pending_tool_calls
                        .get(&cline_tu_id)
                        .cloned()
                        .unwrap_or_default();

                    if devin_calls.is_empty() {
                        // Truly orphaned result: no assistant message in the log declares
                        // this tool_use id (Cline-side compaction dropped the call).
                        // Emitting a `tool` node would leave an unpaired tool_use_id that
                        // the provider rejects, so keep the output as plain user text.
                        let parent =
                            Some(last_tool_result_node.unwrap_or(last_rendered_assistant_node));
                        let content_str = match c.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => serde_json::to_string(v).unwrap_or_default(),
                            None => "undefined".into(),
                        };
                        let text = format!("[tool output]\n{content_str}");
                        push_node(&mut nodes, parent, |nid, p| {
                            build_user_node(nid, p, &text, ts, Vec::new())
                        });
                        tag_last!(m_index);
                        last_tool_result_node = Some(nodes[nodes.len() - 1].node_id);
                        continue;
                    }

                    let shares = tool_result_shares(c, &devin_calls);
                    for (i, call) in devin_calls.iter().enumerate() {
                        let share = &shares[i];

                        let status = if share.success == Some(false) {
                            ToolCallStatus::Error
                        } else {
                            ToolCallStatus::Success
                        };
                        tool_call_outcomes.insert(
                            call.devin.id.clone(),
                            ToolCallOutcome {
                                status,
                                exit_code: None,
                                duration_ms: None,
                            },
                        );

                        let parent =
                            Some(last_tool_result_node.unwrap_or(last_rendered_assistant_node));
                        let arguments = call.devin.arguments.clone();
                        let name = call.devin.name.clone();
                        let call_id = call.devin.id.clone();
                        let share_content = share.content.clone().unwrap_or_default();
                        push_node(&mut nodes, parent, |nid, p| {
                            build_tool_node(
                                nid,
                                p,
                                &call_id,
                                &share_content,
                                &name,
                                &arguments,
                                ts,
                                Some(ToolResultInfo {
                                    status,
                                    exit_code: None,
                                    duration_ms: None,
                                }),
                            )
                        });
                        tag_last!(m_index);
                        last_tool_result_node = Some(nodes[nodes.len() - 1].node_id);
                    }

                    pending_tool_calls.remove(&cline_tu_id);
                }
            }
        } else if role == "assistant" {
            let empty: Vec<Value> = Vec::new();
            let content = m.get("content").and_then(Value::as_array).unwrap_or(&empty);
            let mut text_parts: Vec<String> = Vec::new();
            let mut thinking_text = String::new();
            // Provider seal on the thinking block (`signature`), or the opaque
            // payload of a `redacted_thinking` entry (`data`) — preserved verbatim.
            let mut thinking_signature: Option<String> = None;
            let mut tool_uses: Vec<&Value> = Vec::new();

            for c in content {
                match c.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        // Array.join semantics: null/missing → "".
                        text_parts.push(match c.get("text") {
                            Some(Value::String(s)) => s.clone(),
                            Some(
                                v @ (Value::Bool(_)
                                | Value::Number(_)
                                | Value::Object(_)
                                | Value::Array(_)),
                            ) => js_string(v),
                            _ => String::new(),
                        });
                    }
                    Some("thinking") => {
                        thinking_text = c
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if let Some(sig) = c.get("signature").and_then(Value::as_str) {
                            thinking_signature = Some(sig.to_string());
                        }
                    }
                    Some("redacted_thinking") => {
                        if thinking_text.is_empty() {
                            thinking_text = REDACTED_THINKING.to_string();
                        } else {
                            thinking_text = format!("{thinking_text}\n{REDACTED_THINKING}");
                        }
                        if let Some(data) = c.get("data").and_then(Value::as_str) {
                            thinking_signature = Some(data.to_string());
                        }
                    }
                    Some("tool_use") => tool_uses.push(c),
                    _ => {}
                }
            }

            let text = text_parts.join("\n");
            let mut devin_tool_calls: Vec<ToolCall> = Vec::new();

            for tu in tool_uses {
                let mapped = map_tool_use(tu);
                let mut call_list: Vec<PendingCall> = Vec::new();
                for mc in mapped {
                    let devin_tc = ToolCall {
                        id: make_tool_call_id(),
                        name: mc.name,
                        arguments: mc.arguments,
                        index: i64::try_from(devin_tool_calls.len()).unwrap_or(i64::MAX),
                        kind: "function".into(),
                        status: None,
                        exit_code: None,
                        duration_ms: None,
                        locations: mc.locations.unwrap_or_default(),
                        diffs: mc.diffs.unwrap_or_default(),
                    };
                    devin_tool_calls.push(devin_tc.clone());
                    call_list.push(PendingCall {
                        devin: devin_tc,
                        result_key: mc.result_key,
                    });
                }
                let tu_key = match tu.get("id") {
                    None => "undefined".to_string(),
                    Some(v) => js_string(v),
                };
                pending_tool_calls.insert(tu_key, call_list);
            }

            // Calls from earlier turns stay claimable: an interrupted turn's output can
            // arrive after later turns, and pairing results by id keeps it with the
            // assistant turn that asked for it.

            let parent = Some(last_tool_result_node.unwrap_or(last_rendered_assistant_node));
            let usage = usage_from_cline_metrics(field(m, "metrics"));
            let model = model_from_message(m);
            for rendered in [false, true] {
                push_node(&mut nodes, parent, |nid, p| {
                    build_assistant_node(
                        nid,
                        p,
                        &text,
                        &thinking_text,
                        thinking_signature.clone(),
                        devin_tool_calls.clone(),
                        ts,
                        rendered,
                        usage.clone(),
                        model.clone(),
                    )
                });
                tag_last!(m_index);
            }

            last_rendered_assistant_node = nodes[nodes.len() - 1].node_id;
            last_tool_result_node = None;
        }
    }

    // Result entries carried the success flag per call — fold it back onto the
    // assistant twins' ToolCalls now that all results have been seen.
    let enriched_nodes = shared::apply_tool_call_outcomes(&nodes, &tool_call_outcomes);

    let main_chain_id = enriched_nodes.last().map_or(0, |n| n.node_id);

    let mut prompt_history: Vec<PromptHistoryEntry> = Vec::new();
    for m in messages {
        if m.get("role").and_then(Value::as_str) == Some("user") {
            let empty: Vec<Value> = Vec::new();
            let content = m.get("content").and_then(Value::as_array).unwrap_or(&empty);
            for c in content {
                if c.get("type").and_then(Value::as_str) == Some("text")
                    && c.get("text").is_some_and(js_truthy)
                {
                    let content_text = match c.get("text") {
                        Some(Value::String(s)) => clean_user_text(s),
                        Some(_) => {
                            return Err("cleanUserText: text.startsWith is not a function".into());
                        }
                        None => String::new(),
                    };
                    prompt_history.push(PromptHistoryEntry {
                        content: content_text,
                        timestamp: ts_to_epoch_seconds(field(m, "ts"), created_at) * 1000.0,
                        is_shell: false,
                    });
                }
            }
        }
    }

    let resolved_id = session_id.map_or_else(
        || {
            meta.get("session_id").and_then(Value::as_str).map_or_else(
                || {
                    dir.file_name()
                        .map_or_else(|| dir_str.clone(), |n| n.to_string_lossy().to_string())
                },
                str::to_string,
            )
        },
        str::to_string,
    );
    let subagent = cline_subagent_info(&resolved_id);

    Ok(Session {
        id: resolved_id,
        title,
        working_directory: cwd,
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model,
        created_at,
        last_activity_at,
        main_chain_id,
        shell_last_seen_index: 0,
        cogs_json: shared::default_cogs_json(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: subagent.as_ref().map(|s| s.parent_session_id.clone()),
        agent_id: subagent.as_ref().map(|s| s.agent_id.clone()),
        checkpoints: checkpoints_from_manifest(meta),
        metadata: shared::default_session_metadata(),
        nodes: enriched_nodes,
        prompt_history,
    })
}

fn to_cline_tool_input(tc: &ToolCall) -> Value {
    let arg = |key: &str| tc.arguments.get(key).cloned().unwrap_or(Value::Null);
    match tc.name.as_str() {
        "read" => json!({ "files": [{ "path": arg("file_path") }] }),
        "exec" => json!({ "commands": [arg("command")] }),
        "grep" => json!({ "queries": [arg("pattern")] }),
        "webfetch" => json!({ "requests": [{ "url": arg("url") }] }),
        "edit" => json!({
            "path": arg("file_path"),
            "old_text": arg("old_string"),
            "new_text": arg("new_string"),
        }),
        "write" => json!({
            "path": arg("file_path"),
            "old_text": Value::Null,
            "new_text": arg("content"),
        }),
        _ => tc.arguments.clone(),
    }
}

fn to_cline_tool_name(name: &str) -> &str {
    match name {
        "read" => "read_files",
        "exec" => "run_commands",
        "grep" => "search_codebase",
        "webfetch" => "fetch_web_content",
        "edit" | "write" => "editor",
        other => other,
    }
}

fn to_cline_tool_result_content(node: &MessageNode, tool_name: &str) -> Value {
    if !matches!(
        tool_name,
        "read_files" | "run_commands" | "search_codebase" | "fetch_web_content"
    ) {
        return json!(node.content);
    }

    let args = node
        .metadata
        .as_object()
        .and_then(|m| m.get("toolArguments"))
        .filter(|v| !v.is_null());

    // `args.file_path ?? args.command ?? args.pattern ?? args.url ?? ""` —
    // the first non-nullish key wins; missing keys mean `""`.
    let mut query = Value::Null;
    if let Some(args) = args.and_then(Value::as_object) {
        for key in ["file_path", "command", "pattern", "url"] {
            if let Some(v) = args.get(key).filter(|v| !v.is_null()) {
                query = v.clone();
                break;
            }
        }
    }

    // A call whose arguments carry none of these keys has no query to name it by;
    // the output then stands on its own rather than next to an empty label.
    // (`query.length === 0` in the TS — a non-string payload passes through.)
    if query.is_null() || query.as_str() == Some("") {
        return json!(node.content);
    }

    json!([{
        "query": query,
        "result": node.content,
        "success": node
            .tool_result
            .as_ref()
            .is_none_or(|r| r.status != ToolCallStatus::Error),
    }])
}

fn assistant_fingerprint(node: &MessageNode) -> String {
    json!([
        node.parent_node_id.map_or(Value::Null, |p| json!(p)),
        node.content,
        node.thinking.as_deref().unwrap_or(""),
        node.tool_calls
            .iter()
            .map(|tc| tc.id.as_str())
            .collect::<Vec<_>>(),
    ])
    .to_string()
}

/// Devin records an imported assistant turn twice: an unrendered twin (no row
/// metadata) and a rendered twin (row metadata carries token counts and tool
/// display info). Cline wants exactly one assistant turn per source message, so
/// keep the first node of each twin pair and every assistant that has no twin.
/// System nodes are dropped: Cline rebuilds its own system prompt on resume.
pub fn visible_nodes(session: &Session) -> Vec<MessageNode> {
    let mut seen = HashSet::new();
    session
        .nodes
        .iter()
        .filter(|node| {
            if node.role == Role::System {
                return false;
            }
            if node.role != Role::Assistant {
                return true;
            }
            seen.insert(assistant_fingerprint(node))
        })
        .cloned()
        .collect()
}

pub const CLINE_PROVIDER: &str = "cline-pass";
pub const CLINE_AGENT_VERSION: &str = "3.0.61";
const CLINE_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Cline session ids are `<epoch-ms>_<5 random alphanumerics>`; the id doubles
/// as the session directory name and as the prefix of both artifacts in it.
#[must_use]
pub fn cline_session_id(at_ms: f64) -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let mut suffix = String::with_capacity(5);
    for byte in &bytes[..5] {
        suffix.push(CLINE_ID_ALPHABET[usize::from(byte % CLINE_ID_ALPHABET.len() as u8)] as char);
    }
    format!("{}_{suffix}", at_ms as i64)
}

/// Manifest for a Cline session: the field set the CLI's SessionManifest schema
/// requires in `<session-id>.json`, so the session can be resumed by id.
pub fn session_manifest(session: &Session, session_id: &str, messages_path: &str) -> Value {
    let mut metadata = Map::new();
    metadata.insert("title".into(), json!(session.title));
    // Shadow-git refs the session recorded — preserved verbatim so a
    // Cline→IR→Cline round-trip keeps the checkpoint history. Absent for
    // sessions whose stores never checkpointed.
    if let Some(latest) = session.checkpoints.last() {
        metadata.insert("checkpointEnabled".into(), json!(true));
        metadata.insert(
            "checkpoint".into(),
            json!({ "latest": latest, "history": session.checkpoints }),
        );
    }
    json!({
        "version": 1,
        "session_id": session_id,
        "source": "cli",
        "pid": 0,
        "cwd": session.working_directory,
        "workspace_root": session.working_directory,
        "started_at": to_iso_ms(session.created_at),
        "ended_at": to_iso_ms(session.last_activity_at),
        "status": "completed",
        "exit_code": 0,
        "interactive": true,
        "provider": CLINE_PROVIDER,
        "model": session.model,
        "enable_tools": true,
        "enable_spawn": true,
        "enable_teams": true,
        "prompt": session
            .nodes
            .iter()
            .find(|n| n.role == Role::User)
            .map_or("", |n| n.content.as_str()),
        "metadata": metadata,
        "messages_path": messages_path,
    })
}

/// The placeholder the CLI's own import path (`sanitizeImportedMessages`) writes
/// for a tool call whose result the source log never captured. Sepia writes the
/// same block, so an imported log is what the CLI itself would have produced.
pub const UNCAPTURED_TOOL_RESULT: &str =
    "[import] Tool result was not captured in the source session history.";

fn tool_result_block(node: &MessageNode, name: &str) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": node.tool_call_id.as_deref().unwrap_or(""),
        "name": name,
        "content": to_cline_tool_result_content(node, name),
    })
}

fn uncaptured_tool_result_block(id: &str, name: &str) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": id,
        "name": name,
        "content": UNCAPTURED_TOOL_RESULT,
    })
}

/// Message log for a Cline session, in the CLI's `<session-id>.messages.json` shape.
///
/// The CLI replays the log through the AI SDK, which rejects the transcript as
/// soon as a user turn follows an assistant tool call that is still unresolved
/// (`AI_MissingToolResultsError`). Nodes are therefore not emitted in raw order:
/// a call is paired with its result by tool-call id, results are written directly
/// behind the assistant turn that made the calls, and a call no result was ever
/// recorded for gets a placeholder result.
pub fn session_messages(session: &Session, session_id: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let relevant_nodes = visible_nodes(session);

    let mut calls_by_assistant: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    let mut assistant_by_call_id: HashMap<String, i64> = HashMap::new();
    for node in &relevant_nodes {
        if node.role != Role::Assistant || node.tool_calls.is_empty() {
            continue;
        }
        calls_by_assistant.insert(
            node.node_id,
            node.tool_calls
                .iter()
                .map(|tc| (tc.id.clone(), to_cline_tool_name(&tc.name).to_string()))
                .collect(),
        );
        for tc in &node.tool_calls {
            assistant_by_call_id.insert(tc.id.clone(), node.node_id);
        }
    }

    let mut results_by_call_id: HashMap<String, &MessageNode> = HashMap::new();
    for node in &relevant_nodes {
        if node.role == Role::Tool {
            if let Some(call_id) = &node.tool_call_id {
                if assistant_by_call_id.contains_key(call_id) {
                    results_by_call_id.insert(call_id.clone(), node);
                }
            }
        }
    }

    let mut message_index = 0;
    let mut next_id = || {
        let id = format!("msg_{message_index}");
        message_index += 1;
        id
    };

    for node in &relevant_nodes {
        if node.role == Role::User {
            let mut content: Vec<Value> = vec![json!({ "type": "text", "text": node.content })];
            for block in &node.blocks {
                if let Some(mapped) = to_cline_content_block(block) {
                    content.push(mapped);
                }
            }
            messages.push(json!({
                "id": next_id(),
                "role": "user",
                "content": content,
                "ts": node.created_at * 1000.0,
            }));
            continue;
        }

        if node.role == Role::Assistant {
            let mut content: Vec<Value> = Vec::new();
            if !node.content.is_empty() {
                content.push(json!({ "type": "text", "text": node.content }));
            }
            if let Some(thinking) = &node.thinking {
                match &node.thinking_signature {
                    // A node whose thinking is only the redacted marker + an opaque blob
                    // was a redacted reasoning block — write it back in that form so the
                    // seal replays verbatim instead of signing the marker text.
                    Some(signature) if thinking == REDACTED_THINKING => {
                        content.push(json!({ "type": "redacted_thinking", "data": signature }));
                    }
                    _ => {
                        let mut block = Map::new();
                        block.insert("type".into(), json!("thinking"));
                        block.insert("thinking".into(), json!(thinking));
                        if let Some(signature) = &node.thinking_signature {
                            block.insert("signature".into(), json!(signature));
                        }
                        content.push(Value::Object(block));
                    }
                }
            } else if let Some(signature) = &node.thinking_signature {
                // No text at all — the seal belongs to a fully redacted block.
                content.push(json!({ "type": "redacted_thinking", "data": signature }));
            }
            for tc in &node.tool_calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": to_cline_tool_name(&tc.name),
                    "input": to_cline_tool_input(tc),
                }));
            }
            // A node with nothing in it carries nothing: the CLI's own import path
            // drops such a turn, so sepia does not write one either.
            if content.is_empty() {
                continue;
            }
            let usage = &node.usage;
            messages.push(json!({
                "id": next_id(),
                "role": "assistant",
                "content": content,
                "ts": node.created_at * 1000.0,
                "modelInfo": {
                    "id": node.model.as_deref().unwrap_or(&session.model),
                    "provider": CLINE_PROVIDER,
                },
                "metrics": {
                    "inputTokens": usage.as_ref().map_or(0.0, |u| u.input),
                    "outputTokens": usage.as_ref().map_or(0.0, |u| u.output),
                    "cacheReadTokens": usage.as_ref().and_then(|u| u.cache_read).unwrap_or(0.0),
                    "cacheWriteTokens": usage.as_ref().and_then(|u| u.cache_write).unwrap_or(0.0),
                    "cost": usage.as_ref().and_then(|u| u.cost).unwrap_or(0.0),
                },
            }));

            if let Some(calls) = calls_by_assistant.get(&node.node_id) {
                let results: Vec<Value> = calls
                    .iter()
                    .map(|(call_id, call_name)| {
                        let Some(result) = results_by_call_id.get(call_id) else {
                            return uncaptured_tool_result_block(call_id, call_name);
                        };
                        let name =
                            to_cline_tool_name(result.tool_name.as_deref().unwrap_or(call_name));
                        tool_result_block(result, name)
                    })
                    .collect();
                messages.push(json!({
                    "id": next_id(),
                    "role": "user",
                    "content": results,
                    "ts": node.created_at * 1000.0 + 1.0,
                }));
            }
            continue;
        }

        // A result no visible assistant turn claims cannot be paired with a call:
        // keep it as its own block, which the CLI's log format tolerates. Results a
        // visible call owns were already written behind their assistant turn.
        if let Some(call_id) = &node.tool_call_id {
            if assistant_by_call_id.contains_key(call_id) {
                continue;
            }
        }
        let tool_name = to_cline_tool_name(node.tool_name.as_deref().unwrap_or("unknown"));
        messages.push(json!({
            "id": next_id(),
            "role": "user",
            "content": [tool_result_block(node, tool_name)],
            "ts": node.created_at * 1000.0,
        }));
    }

    json!({
        "version": 1,
        "updated_at": to_iso_ms(session.last_activity_at),
        "agent": "lead",
        "sessionId": session_id,
        "origin": {
            "source": "cli",
            "mode": "user",
            "sessionId": session_id,
            "version": CLINE_AGENT_VERSION,
        },
        "messages": messages,
    })
}

/// Where in the log a violation landed — `"eof"` when calls stay open at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViolationIndex {
    Index(usize),
    Eof,
}

/// A transcript rule violation — a non-result turn that arrived while calls
/// were unresolved, or calls still open at end of log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptViolation {
    pub index: ViolationIndex,
    pub tool_call_ids: Vec<String>,
}

/// The rule the Cline CLI enforces when it replays a session: a turn that is not
/// a tool result must not arrive while an assistant tool call is unresolved — the
/// AI SDK raises `AI_MissingToolResultsError`, and the CLI reports it as "tool
/// results are missing" — and no call may stay unresolved at the end of the log.
/// A transcript `sessionMessages` produced reports no violation here.
fn violation_blocks(message: &Value) -> Vec<&Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items.iter().filter(|b| b.is_object()).collect()
        })
}

pub fn transcript_violations(messages: &[Value]) -> Vec<TranscriptViolation> {
    let blocks_of = violation_blocks;

    let mut pending: HashSet<String> = HashSet::new();
    let mut violations: Vec<TranscriptViolation> = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            for block in blocks_of(message) {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        pending.insert(id.to_string());
                    }
                }
            }
            continue;
        }

        let blocks = blocks_of(message);
        let results: Vec<&&Value> = blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            .collect();
        let result_only = !results.is_empty() && results.len() == blocks.len();
        if !result_only && !pending.is_empty() {
            let mut ids: Vec<String> = pending.iter().cloned().collect();
            ids.sort();
            violations.push(TranscriptViolation {
                index: ViolationIndex::Index(index),
                tool_call_ids: ids,
            });
        }
        for block in results {
            if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                pending.remove(id);
            }
        }
    }

    if !pending.is_empty() {
        let mut ids: Vec<String> = pending.iter().cloned().collect();
        ids.sort();
        violations.push(TranscriptViolation {
            index: ViolationIndex::Eof,
            tool_call_ids: ids,
        });
    }

    violations
}

/// Writes the `<id>.json` manifest and `<id>.messages.json` transcript pair.
fn write_session_files(
    session_id: &str,
    out_dir: &Path,
    manifest: &Value,
    messages: &Value,
) -> Result<(), ConversionError> {
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    std::fs::write(out_dir.join(format!("{session_id}.json")), pretty(manifest)).map_err(|e| {
        ConversionError {
            message: format!("Cline export failed: {e}"),
            cause: Value::Null,
        }
    })?;
    std::fs::write(
        out_dir.join(format!("{session_id}.messages.json")),
        pretty(messages),
    )
    .map_err(|e| ConversionError {
        message: format!("Cline export failed: {e}"),
        cause: Value::Null,
    })?;
    Ok(())
}

/// Reads an existing export back; a missing manifest means nothing to replace.
fn read_session_files(session_id: &str, out_dir: &Path) -> Option<(String, String)> {
    let manifest_path = out_dir.join(format!("{session_id}.json"));
    if !manifest_path.exists() {
        return None;
    }
    let manifest = std::fs::read_to_string(manifest_path).ok()?;
    let messages = std::fs::read_to_string(out_dir.join(format!("{session_id}.messages.json")))
        .unwrap_or_else(|_| "{}".into());
    Some((manifest, messages))
}

/// What `to_directory` resolved to per file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportAction {
    Created,
    Kept,
    Replaced,
    Planned,
}

/// Export a Devin session to a directory as Cline session files (`<id>.json`
/// plus `<id>.messages.json`). Existing files are never replaced: without
/// `force` the export reports them and leaves them untouched, which makes
/// re-running an export idempotent. With `dry_run` it only validates and reports.
///
/// # Errors
/// `ConversionError` on any filesystem failure.
pub fn to_directory(
    session: &Session,
    out_dir: &Path,
    force: bool,
    dry_run: bool,
) -> Result<[ExportAction; 2], ConversionError> {
    let fail = |e: std::io::Error| ConversionError {
        message: format!("Cline export failed: {e}"),
        cause: Value::Null,
    };
    let messages_path = out_dir.join(format!("{}.messages.json", session.id));
    let previous = read_session_files(&session.id, out_dir);
    if previous.is_some() && !force {
        // Diagnostics go to stderr — stdout is the driver-wire channel.
        eprintln!(
            "Session {} already exists in {}; leaving it untouched",
            session.id,
            out_dir.display()
        );
        return Ok([ExportAction::Kept, ExportAction::Kept]);
    }

    if dry_run {
        eprintln!(
            "Would export session {} to {}",
            session.id,
            out_dir.display()
        );
        return Ok([ExportAction::Planned, ExportAction::Planned]);
    }

    std::fs::create_dir_all(out_dir).map_err(fail)?;
    write_session_files(
        &session.id,
        out_dir,
        &session_manifest(session, &session.id, &messages_path.to_string_lossy()),
        &session_messages(session, &session.id),
    )?;

    if previous.is_some() {
        return Ok([ExportAction::Replaced, ExportAction::Replaced]);
    }
    Ok([ExportAction::Created, ExportAction::Created])
}
