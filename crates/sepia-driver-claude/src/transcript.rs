//! Claude Code JSONL transcript reader/writer.
//!
//! Claude Code keeps one append-only JSONL transcript per session at
//! `~/.claude/projects/<slug>/<session-uuid>.jsonl`, where `<slug>` is the
//! working directory with non-alphanumeric characters replaced by `-`.
//! Sub-agent (Task tool) transcripts live in `<uuid>/subagents/agent-*.jsonl`
//! (current layout) or as `agent-*.jsonl` siblings (legacy layout); every
//! entry in them carries `isSidechain: true` and the parent's `sessionId`.
//!
//! This module reads that format into the session IR and writes it back via
//! [`to_jsonl`]: one entry per IR node, chained by `parentUuid`, so a written
//! transcript resumes with `claude --resume <session-id>`. Sealed thinking
//! (`signature`/`redacted_thinking data`) echoes back; unsigned thinking is
//! dropped on write, matching the other writers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use sepia_core::domain::{
    Block, ConversionError, MessageNode, PromptHistoryEntry, REDACTED_THINKING, Role, Session,
    TokenUsage, ToolCall, ToolCallDiff, ToolCallLocation, ToolCallStatus,
};
use sepia_core::restore::FILE_HISTORY_KIND;
use sepia_core::shared::{self, decode_project_dir};

const JSONL: &str = ".jsonl";

fn as_map(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn finite_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&ch| {
            let code = ch as u32;
            code >= 32 || ch == '\t' || ch == '\n' || ch == '\r'
        })
        .collect()
}

/// `timestamp` is ISO-8601 with millis; the IR keeps epoch seconds.
/// JS `new Date(value)` semantics: RFC 3339 strings parse, anything else is
/// `None`.
fn to_seconds(value: &Value) -> Option<f64> {
    let text = value.as_str()?;
    let parsed = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    let millis = parsed.unix_timestamp_nanos() as f64 / 1e6;
    Some((millis / 1000.0).floor())
}

/// Epoch seconds → `new Date(ms).toISOString()` (`YYYY-MM-DDTHH:MM:SS.sssZ`).
fn to_iso_millis(seconds: f64) -> String {
    let nanos = (seconds * 1e9) as i128;
    let Ok(dt) = OffsetDateTime::from_unix_timestamp_nanos(nanos) else {
        return "1970-01-01T00:00:00.000Z".to_string();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond(),
    )
}

/// A `message.content` array item mapped onto the IR block union.
/// `tool_use`/`tool_result`/`thinking` have dedicated IR fields and are
/// skipped here; `image`/`document` are the attachment forms (`source`
/// carries `base64`/`url`/`text` variants).
fn block_from_claude(item: &Value) -> Option<Block> {
    let item = as_map(item)?;
    let source = item.get("source").and_then(as_map);
    let source_field = |key: &str| source.and_then(|s| str_field(s, key));
    match str_field(item, "type") {
        Some("text") => Some(Block::Text {
            text: str_field(item, "text")?.to_string(),
        }),
        Some("image") => {
            let data = str_field(item, "data").or_else(|| source_field("data"));
            let uri = str_field(item, "url").or_else(|| source_field("url"));
            if data.is_none() && uri.is_none() {
                return None;
            }
            let mime_type = source_field("media_type")
                .or_else(|| str_field(item, "media_type"))
                .or_else(|| str_field(item, "mimeType"));
            Some(Block::Image {
                data: data.map(str::to_string),
                mime_type: mime_type.map(str::to_string),
                uri: uri.map(str::to_string),
            })
        }
        Some("document") => {
            let text = source_field("text").or_else(|| str_field(item, "text"));
            let data = source_field("data").or_else(|| str_field(item, "data"));
            let uri = source_field("url").or_else(|| str_field(item, "url"));
            if text.is_none() && data.is_none() && uri.is_none() {
                return None;
            }
            let mime_type = source_field("media_type")
                .or_else(|| str_field(item, "media_type"))
                .or_else(|| str_field(item, "mimeType"));
            Some(Block::File {
                uri: uri.map(str::to_string),
                name: str_field(item, "title").map(str::to_string),
                mime_type: mime_type.map(str::to_string),
                size: None,
                text: text.map(str::to_string),
                data: data.map(str::to_string),
            })
        }
        _ => None,
    }
}

/// Claude's per-message `usage` — `input_tokens`/`output_tokens` plus cache
/// tiers (`cache_read_input_tokens`, `cache_creation_input_tokens` or the
/// nested `cache_creation.ephemeral_*` split) — onto IR `TokenUsage`.
pub fn usage_from_claude(usage: &Value) -> Option<TokenUsage> {
    let usage = as_map(usage)?;
    let input = usage.get("input_tokens").and_then(finite_number);
    let output = usage.get("output_tokens").and_then(finite_number);
    if input.is_none() && output.is_none() {
        return None;
    }
    let cache_read = usage.get("cache_read_input_tokens").and_then(finite_number);
    let creation = usage.get("cache_creation").and_then(as_map);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .and_then(finite_number)
        .or_else(|| {
            creation.map(|c| {
                c.get("ephemeral_5m_input_tokens")
                    .and_then(finite_number)
                    .unwrap_or(0.0)
                    + c.get("ephemeral_1h_input_tokens")
                        .and_then(finite_number)
                        .unwrap_or(0.0)
            })
        });
    Some(TokenUsage {
        input: input.unwrap_or(0.0),
        output: output.unwrap_or(0.0),
        cache_read,
        cache_write,
        thinking: None,
        cost: None,
    })
}

/// A `tool_result` block's `content` is a string or a block array.
fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let parts: Vec<&str> = items
                .iter()
                .filter_map(|item| {
                    let item = as_map(item)?;
                    (str_field(item, "type") == Some("text"))
                        .then(|| str_field(item, "text"))
                        .flatten()
                })
                .collect();
            if parts.is_empty() {
                serde_json::to_string(&Value::Array(items.clone())).unwrap_or_default()
            } else {
                parts.join("\n")
            }
        }
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The text a `user` entry carries, or `None` for tool-result-only/meta-less
/// entries.
fn user_entry_text(entry: &Map<String, Value>) -> Option<String> {
    let content = entry.get("message").and_then(as_map)?.get("content")?;
    match content {
        Value::String(s) => (!s.is_empty()).then(|| s.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    let item = as_map(item)?;
                    (str_field(item, "type") == Some("text"))
                        .then(|| str_field(item, "text"))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// File paths a `tool_use` input names — the `locations`/`diffs` projection
/// that makes a recorded edit revertable. `Edit`/`MultiEdit` inputs carry the
/// before/after hunks verbatim and `Write` records the written content — a
/// create-revert delete. Read-style tools (`Read`, `Glob`, `Grep`, `LS`,
/// `NotebookEdit`) contribute locations only; `NotebookEdit`'s `new_source`
/// is cell-level, not a file diff.
pub fn tool_file_refs(name: &str, input: &Value) -> (Vec<ToolCallLocation>, Vec<ToolCallDiff>) {
    let empty = Map::new();
    let obj = as_map(input).unwrap_or(&empty);
    let path = str_field(obj, "file_path")
        .or_else(|| str_field(obj, "path"))
        .or_else(|| str_field(obj, "notebook_path"));
    let locations: Vec<ToolCallLocation> = match path {
        Some(p) if !p.is_empty() => vec![ToolCallLocation {
            path: p.to_string(),
            line: None,
        }],
        _ => Vec::new(),
    };
    let hunk = |record: &Map<String, Value>| -> Vec<ToolCallDiff> {
        let Some(path) = path else { return Vec::new() };
        let old_text = str_field(record, "old_string");
        let new_text = str_field(record, "new_string");
        if old_text.is_none() && new_text.is_none() {
            return Vec::new();
        }
        vec![ToolCallDiff {
            path: path.to_string(),
            old_text: old_text.map(str::to_string),
            new_text: new_text.map(str::to_string),
        }]
    };
    match name {
        "Edit" => (locations, hunk(obj)),
        "MultiEdit" => {
            let diffs = obj
                .get("edits")
                .and_then(Value::as_array)
                .map(|edits| {
                    edits
                        .iter()
                        .filter_map(|e| as_map(e).map(&hunk))
                        .flatten()
                        .collect()
                })
                .unwrap_or_default();
            (locations, diffs)
        }
        "Write" => {
            let diffs = match (path, str_field(obj, "content")) {
                (Some(path), Some(content)) => vec![ToolCallDiff {
                    path: path.to_string(),
                    old_text: None,
                    new_text: Some(content.to_string()),
                }],
                _ => Vec::new(),
            };
            (locations, diffs)
        }
        _ => (locations, Vec::new()),
    }
}

/// Recorded `locations` a written `tool_use` item carries (the driver's
/// lossless round-trip slot — real Claude transcripts never have one).
fn recorded_locations(item: &Map<String, Value>) -> Option<Vec<ToolCallLocation>> {
    let items = item.get("locations")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|loc| {
                let loc = as_map(loc)?;
                Some(ToolCallLocation {
                    path: str_field(loc, "path")?.to_string(),
                    line: loc.get("line").and_then(Value::as_i64),
                })
            })
            .collect(),
    )
}

/// Recorded `diffs` a written `tool_use` item carries (see
/// [`recorded_locations`]).
fn recorded_diffs(item: &Map<String, Value>) -> Option<Vec<ToolCallDiff>> {
    let items = item.get("diffs")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|diff| {
                let diff = as_map(diff)?;
                Some(ToolCallDiff {
                    path: str_field(diff, "path")?.to_string(),
                    old_text: diff
                        .get("oldText")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    new_text: diff
                        .get("newText")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect(),
    )
}

/// What a file's lines prove about the session, without building nodes.
#[derive(Default)]
struct SessionMeta {
    title: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    git_branch: Option<String>,
    claude_version: Option<String>,
    slug: Option<String>,
    /// `sessionId` the entries name — the parent's id inside a subagent file.
    session_id: Option<String>,
    /// `agentId` the entries name — set on every line of a subagent file.
    agent_id: Option<String>,
    permission_mode: Option<String>,
    created_at: Option<f64>,
    last_activity_at: Option<f64>,
    saw_sidechain: bool,
    first_user_text: Option<String>,
    prompt_history: Vec<PromptHistoryEntry>,
    checkpoints: Vec<sepia_core::domain::CheckpointRef>,
    /// `ref` → snapshot payload, keyed the same as `checkpoints`.
    /// `preserve_order` keeps insertion order for stable wire output.
    file_history: Map<String, Value>,
}

fn collect_meta(entries: &[Map<String, Value>]) -> SessionMeta {
    let mut meta = SessionMeta::default();

    for entry in entries {
        let ts = to_seconds(entry.get("timestamp").unwrap_or(&Value::Null));
        if let Some(ts) = ts {
            meta.created_at.get_or_insert(ts);
            meta.last_activity_at = Some(ts);
        }
        if meta.cwd.is_none() {
            meta.cwd = str_field(entry, "cwd").map(str::to_string);
        }
        if meta.session_id.is_none() {
            meta.session_id = str_field(entry, "sessionId").map(str::to_string);
        }
        if meta.agent_id.is_none() {
            meta.agent_id = str_field(entry, "agentId").map(str::to_string);
        }
        if meta.permission_mode.is_none() {
            meta.permission_mode = str_field(entry, "permissionMode").map(str::to_string);
        }
        if meta.claude_version.is_none() {
            meta.claude_version = str_field(entry, "version").map(str::to_string);
        }
        if meta.slug.is_none() {
            meta.slug = str_field(entry, "slug").map(str::to_string);
        }
        if let Some(branch) = str_field(entry, "gitBranch") {
            meta.git_branch = Some(branch.to_string());
        }
        if entry.get("isSidechain") == Some(&Value::Bool(true)) {
            meta.saw_sidechain = true;
        }

        if str_field(entry, "type") == Some("file-history-snapshot") {
            // The workspace-state checkpoint the JSONL records per turn — the
            // `ref` (messageId) pins a path→backup map; the blobs themselves
            // live under `~/.claude/file-history/<sessionId>/`, resolved at
            // restore time. `isSnapshotUpdate` entries top up an earlier ref —
            // merge their files rather than appending a second ref for the
            // same snapshot.
            let empty = Map::new();
            let snapshot = entry.get("snapshot").and_then(as_map).unwrap_or(&empty);
            let ref_ = str_field(entry, "messageId")
                .or_else(|| str_field(snapshot, "messageId"))
                .or_else(|| str_field(entry, "uuid"));
            let Some(ref_) = ref_ else { continue };
            let at = (to_seconds(snapshot.get("timestamp").unwrap_or(&Value::Null))
                .or(ts)
                .or(meta.created_at)
                .unwrap_or(0.0))
                * 1000.0;
            let mut files = meta
                .file_history
                .get(ref_)
                .and_then(|snapshot| snapshot.get("files"))
                .and_then(as_map)
                .cloned()
                .unwrap_or_default();
            if let Some(tracked) = snapshot.get("trackedFileBackups").and_then(as_map) {
                for (file_path, backup) in tracked {
                    let Some(backup) = as_map(backup) else {
                        continue;
                    };
                    let mut entry_value = Map::new();
                    entry_value.insert(
                        "backup".into(),
                        backup
                            .get("backupFileName")
                            .and_then(Value::as_str)
                            .map_or(Value::Null, |s| json!(s)),
                    );
                    if let Some(version) = backup.get("version").and_then(finite_number) {
                        entry_value.insert("version".into(), json!(version));
                    }
                    files.insert(file_path.clone(), Value::Object(entry_value));
                }
            }
            if !meta.file_history.contains_key(ref_) {
                meta.checkpoints.push(sepia_core::domain::CheckpointRef {
                    r#ref: ref_.to_string(),
                    created_at: at,
                    run_count: None,
                    kind: Some(FILE_HISTORY_KIND.to_string()),
                });
            }
            // `isSnapshotUpdate` entries merge their files into the existing
            // payload and refresh `at` — same ref, no second checkpoint.
            meta.file_history
                .insert(ref_.to_string(), json!({ "at": at, "files": files }));
            continue;
        }

        match str_field(entry, "type") {
            Some("summary") => {
                if meta.title.is_none() {
                    meta.title = str_field(entry, "summary").map(str::to_string);
                }
            }
            Some("assistant") => {
                if let Some(message) = entry.get("message").and_then(as_map) {
                    if let Some(model) = str_field(message, "model") {
                        meta.model = Some(model.to_string());
                    }
                }
            }
            Some("user") => {
                let Some(text) = user_entry_text(entry) else {
                    continue;
                };
                if meta.first_user_text.is_none() {
                    meta.first_user_text = Some(text.clone());
                }
                // `isMeta` lines are command plumbing (`/clear`, local-command
                // output), not prompts the user typed.
                if entry.get("isMeta") == Some(&Value::Bool(true)) {
                    continue;
                }
                meta.prompt_history.push(PromptHistoryEntry {
                    content: sanitize(&text),
                    timestamp: (ts.or(meta.created_at).unwrap_or(0.0)) * 1000.0,
                    is_shell: false,
                });
            }
            _ => {}
        }
    }

    meta
}

/// Provenance flags worth keeping on the node — everything else the entry
/// carried is noise.
fn node_meta(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert(
        "uuid".into(),
        str_field(entry, "uuid").map_or(Value::Null, |s| json!(s)),
    );
    for flag in [
        "isSidechain",
        "isMeta",
        "isCompactSummary",
        "isVisibleInTranscriptOnly",
    ] {
        if entry.get(flag) == Some(&Value::Bool(true)) {
            meta.insert(flag.into(), Value::Bool(true));
        }
    }
    meta
}

/// The `parentUuid`/`uuid` chain onto `parent_node_id`/`node_id`. Every entry
/// records its own parent link — even types that emit no node
/// (`file-history-snapshot`, `queue-operation`, `progress`) — so resolving a
/// parent walks up through skipped entries to the nearest emitted node. An
/// explicit `null` starts a new root (sidechain roots inside a mixed legacy
/// file); a dangling id falls back to the previous node, which is what the
/// append order already gives a linear thread.
fn build_nodes(entries: &[Map<String, Value>], default_ts: f64) -> Vec<MessageNode> {
    let mut nodes: Vec<MessageNode> = Vec::new();
    let mut node_id_by_uuid: HashMap<String, i64> = HashMap::new();
    let mut parent_by_uuid: HashMap<String, Value> = HashMap::new();
    let mut tool_name_by_id: HashMap<String, String> = HashMap::new();
    let mut tool_args_by_id: HashMap<String, Value> = HashMap::new();
    let mut last_node_id: Option<i64> = None;

    // `parentUuid` semantics: explicit `null` starts a new root (sidechain
    // roots); a string walks the uuid→parent map through skipped entries;
    // absent/non-string/dangling values fall back to the previous node so
    // the append order stays a linear thread.
    macro_rules! resolve_parent {
        ($parent_uuid:expr) => {{
            let parent_uuid: Option<&Value> = $parent_uuid;
            let resolved: Option<Option<i64>> = match parent_uuid {
                None => None,
                Some(v) if v.is_null() => Some(None),
                Some(v) if v.is_string() => {
                    let mut cursor = v.clone();
                    let mut seen: HashSet<String> = HashSet::new();
                    let mut found = None;
                    while let Some(key) = cursor.as_str() {
                        if seen.contains(key) {
                            break;
                        }
                        if let Some(node_id) = node_id_by_uuid.get(key) {
                            found = Some(*node_id);
                            break;
                        }
                        seen.insert(key.to_string());
                        cursor = parent_by_uuid.get(key).cloned().unwrap_or(Value::Null);
                    }
                    found.map(Some)
                }
                Some(_) => None,
            };
            match resolved {
                Some(parent) => parent,
                // Absent or dangling: keep the append-order chain going.
                None => last_node_id,
            }
        }};
    }

    for entry in entries {
        if let Some(uuid) = str_field(entry, "uuid") {
            parent_by_uuid.insert(
                uuid.to_string(),
                entry.get("parentUuid").cloned().unwrap_or(Value::Null),
            );
        }
        let ts = to_seconds(entry.get("timestamp").unwrap_or(&Value::Null)).unwrap_or(default_ts);
        let null = Value::Null;

        macro_rules! push {
            ($entry:expr, $make:expr) => {{
                let node_id = i64::try_from(nodes.len()).unwrap_or(i64::MAX);
                let parent = resolve_parent!($entry.get("parentUuid"));
                let node: MessageNode = $make(node_id, parent);
                nodes.push(node);
                last_node_id = Some(node_id);
                if let Some(uuid) = str_field($entry, "uuid") {
                    node_id_by_uuid.insert(uuid.to_string(), node_id);
                }
            }};
        }

        match str_field(entry, "type") {
            Some("user") => {
                let content = entry
                    .get("message")
                    .and_then(as_map)
                    .and_then(|m| m.get("content"));
                match content {
                    Some(Value::String(content)) => {
                        // Empty `content` strings are plumbing entries (meta
                        // commands, continuation markers), not messages.
                        if content.is_empty() {
                            continue;
                        }
                        let text = sanitize(content);
                        let metadata = Value::Object(node_meta(entry));
                        push!(entry, |node_id, parent| MessageNode {
                            node_id,
                            parent_node_id: parent,
                            role: Role::User,
                            content: text.clone(),
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
                            created_at: ts,
                            metadata: metadata.clone(),
                        });
                    }
                    Some(Value::Array(items)) => {
                        let mut text_parts: Vec<String> = Vec::new();
                        let mut blocks: Vec<Block> = Vec::new();
                        let mut results: Vec<&Map<String, Value>> = Vec::new();
                        for item in items {
                            if let Some(obj) = as_map(item) {
                                if str_field(obj, "type") == Some("tool_result") {
                                    results.push(obj);
                                    continue;
                                }
                            }
                            let Some(block) = block_from_claude(item) else {
                                continue;
                            };
                            if let Block::Text { text } = &block {
                                text_parts.push(text.clone());
                            }
                            blocks.push(block);
                        }
                        if !blocks.is_empty() {
                            let keep_blocks =
                                blocks.iter().any(|b| !matches!(b, Block::Text { .. }));
                            let content_text = sanitize(&text_parts.join("\n"));
                            let metadata = Value::Object(node_meta(entry));
                            push!(entry, |node_id, parent| MessageNode {
                                node_id,
                                parent_node_id: parent,
                                role: Role::User,
                                content: content_text.clone(),
                                blocks: if keep_blocks {
                                    blocks.clone()
                                } else {
                                    Vec::new()
                                },
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
                                created_at: ts,
                                metadata: metadata.clone(),
                            });
                        }
                        for result in results {
                            let call_id = str_field(result, "tool_use_id");
                            let status = if result.get("is_error") == Some(&Value::Bool(true)) {
                                ToolCallStatus::Error
                            } else {
                                ToolCallStatus::Success
                            };
                            let content_text =
                                sanitize(&tool_result_text(result.get("content").unwrap_or(&null)));
                            let mut metadata = node_meta(entry);
                            metadata.insert(
                                "toolArguments".into(),
                                call_id
                                    .and_then(|id| tool_args_by_id.get(id).cloned())
                                    .unwrap_or(Value::Null),
                            );
                            metadata.insert(
                                "toolUseResult".into(),
                                entry.get("toolUseResult").cloned().unwrap_or(Value::Null),
                            );
                            let tool_name = call_id.and_then(|id| tool_name_by_id.get(id).cloned());
                            let call_id = call_id.map(str::to_string);
                            push!(entry, |node_id, parent| MessageNode {
                                node_id,
                                parent_node_id: parent,
                                role: Role::Tool,
                                content: content_text.clone(),
                                blocks: Vec::new(),
                                tool_calls: Vec::new(),
                                tool_call_id: call_id.clone(),
                                tool_name: tool_name.clone(),
                                thinking: None,
                                thinking_signature: None,
                                usage: None,
                                model: None,
                                request_id: None,
                                finish_reason: None,
                                tool_result: Some(sepia_core::domain::ToolResultInfo {
                                    status,
                                    exit_code: None,
                                    duration_ms: None,
                                }),
                                created_at: ts,
                                metadata: Value::Object(metadata.clone()),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                let empty = Map::new();
                let message = entry.get("message").and_then(as_map).unwrap_or(&empty);
                let content = message
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut text_parts: Vec<String> = Vec::new();
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<ToolCall> = Vec::new();
                // Provider seal of the thinking payload — `signature` on
                // `thinking` blocks, `data` on `redacted_thinking`; the last
                // one wins when several sealed blocks fold into the single
                // `thinking` projection.
                let mut thinking_signature: Option<String> = None;
                for item in &content {
                    let Some(item) = as_map(item) else {
                        continue;
                    };
                    match str_field(item, "type") {
                        Some("text") => {
                            if let Some(text) = str_field(item, "text") {
                                text_parts.push(text.to_string());
                            }
                        }
                        Some("thinking") => {
                            if let Some(thinking) = str_field(item, "thinking") {
                                thinking_parts.push(thinking.to_string());
                            }
                            if let Some(signature) = str_field(item, "signature") {
                                thinking_signature = Some(signature.to_string());
                            }
                        }
                        // `redacted_thinking` is an opaque blob — no text to
                        // keep, so the marker stands in and `data` rides as
                        // the signature.
                        Some("redacted_thinking") => {
                            thinking_parts.push(REDACTED_THINKING.to_string());
                            if let Some(data) = str_field(item, "data") {
                                thinking_signature = Some(data.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let id = str_field(item, "id").map_or_else(
                                || format!("claude-tool-{}-{}", nodes.len(), tool_calls.len()),
                                str::to_string,
                            );
                            let name = str_field(item, "name").unwrap_or("unknown").to_string();
                            let args = item.get("input").cloned().unwrap_or_else(|| json!({}));
                            // A transcript this driver wrote may carry the
                            // recorded IR `locations`/`diffs` — prefer them so
                            // a save→read round-trip is lossless; real Claude
                            // transcripts never have them and derive instead.
                            let (locations, diffs) = {
                                let (derived_locations, derived_diffs) =
                                    tool_file_refs(&name, &args);
                                (
                                    recorded_locations(item).unwrap_or(derived_locations),
                                    recorded_diffs(item).unwrap_or(derived_diffs),
                                )
                            };
                            tool_name_by_id.insert(id.clone(), name.clone());
                            tool_args_by_id.insert(id.clone(), args.clone());
                            tool_calls.push(ToolCall {
                                id,
                                name,
                                arguments: args,
                                index: i64::try_from(tool_calls.len()).unwrap_or(i64::MAX),
                                kind: "function".into(),
                                status: None,
                                exit_code: None,
                                duration_ms: None,
                                locations,
                                diffs,
                            });
                        }
                        _ => {}
                    }
                }
                let thinking = thinking_parts.join("\n");
                let usage = usage_from_claude(message.get("usage").unwrap_or(&null));
                let model = str_field(message, "model").map(str::to_string);
                let request_id = str_field(entry, "requestId").map(str::to_string);
                let finish_reason = str_field(message, "stop_reason").map(str::to_string);
                let mut metadata = node_meta(entry);
                metadata.insert(
                    "messageId".into(),
                    str_field(message, "id").map_or(Value::Null, |s| json!(s)),
                );
                push!(entry, |node_id, parent| MessageNode {
                    node_id,
                    parent_node_id: parent,
                    role: Role::Assistant,
                    content: sanitize(&text_parts.join("\n")),
                    blocks: Vec::new(),
                    tool_calls: tool_calls.clone(),
                    tool_call_id: None,
                    tool_name: None,
                    thinking: (!thinking.is_empty()).then(|| sanitize(&thinking)),
                    thinking_signature: thinking_signature.clone(),
                    usage: usage.clone(),
                    model: model.clone(),
                    request_id: request_id.clone(),
                    finish_reason: finish_reason.clone(),
                    tool_result: None,
                    created_at: ts,
                    metadata: Value::Object(metadata.clone()),
                });
            }
            Some("system") => {
                let subtype = str_field(entry, "subtype");
                let content = str_field(entry, "content").map_or_else(
                    || format!("[claude {}]", subtype.unwrap_or("system")),
                    sanitize,
                );
                let mut metadata = node_meta(entry);
                metadata.insert("subtype".into(), subtype.map_or(Value::Null, |s| json!(s)));
                metadata.insert(
                    "level".into(),
                    str_field(entry, "level").map_or(Value::Null, |s| json!(s)),
                );
                metadata.insert(
                    "compactMetadata".into(),
                    entry.get("compactMetadata").cloned().unwrap_or(Value::Null),
                );
                push!(entry, |node_id, parent| MessageNode {
                    node_id,
                    parent_node_id: parent,
                    role: Role::System,
                    content: content.clone(),
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
                    created_at: ts,
                    metadata: Value::Object(metadata.clone()),
                });
            }
            // `summary`, `file-history-snapshot`, `queue-operation` and
            // unknown types emit no node — their uuid/parentUuid are still
            // recorded above so a later entry's parent link resolves through
            // them.
            _ => {}
        }
    }

    nodes
}

/// What the file's location says — the repo passes the parts it knows.
#[derive(Clone, Debug, Default)]
pub struct ClaudeSourceInfo {
    /// Session id — the JSONL file's basename without the `.jsonl` extension.
    pub id: String,
    /// Working directory to use when no entry records `cwd` (a decoded slug).
    pub fallback_cwd: Option<String>,
    /// Parent session id when the layout proves it (`<uuid>/subagents/` files).
    pub parent_session_id: Option<String>,
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64().floor())
}

fn session_from(meta: SessionMeta, source: &ClaudeSourceInfo, nodes: Vec<MessageNode>) -> Session {
    let now = now_seconds();
    let created_at = meta.created_at.unwrap_or(now);
    let title = meta
        .title
        .or_else(|| {
            meta.first_user_text.map(|text| {
                text.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(80)
                    .collect()
            })
        })
        .unwrap_or_else(|| source.id.clone());
    // A subagent file's entries name the parent session (`sessionId`) and mark
    // themselves `isSidechain`; a main file's sidechain-free entries name the
    // file itself, so a mismatch is only trusted once a sidechain was seen.
    let sidechain_parent = match (meta.saw_sidechain, meta.session_id.as_deref()) {
        (true, Some(session_id)) if session_id != source.id => Some(session_id.to_string()),
        _ => None,
    };
    let agent_id = meta
        .agent_id
        .clone()
        .or_else(|| source.id.strip_prefix("agent-").map(str::to_string));

    let mut metadata = Map::new();
    metadata.insert("source".into(), json!("claude-code"));
    metadata.insert(
        "gitBranch".into(),
        meta.git_branch.map_or(Value::Null, Value::String),
    );
    metadata.insert(
        "claudeVersion".into(),
        meta.claude_version.map_or(Value::Null, Value::String),
    );
    metadata.insert("slug".into(), meta.slug.map_or(Value::Null, Value::String));
    if !meta.file_history.is_empty() {
        // The `file-history/<sessionId>/` dir a checkpoint restore resolves
        // backup names against — `sessionId` on the entries names the owning
        // session even inside a subagent file.
        let mut snapshots = Map::new();
        for (ref_, snapshot) in meta.file_history {
            snapshots.insert(ref_, snapshot);
        }
        metadata.insert(
            "fileHistory".into(),
            json!({
                "sessionId": meta.session_id.unwrap_or_else(|| source.id.clone()),
                "snapshots": snapshots,
            }),
        );
    }

    Session {
        id: source.id.clone(),
        title,
        working_directory: meta
            .cwd
            .or_else(|| source.fallback_cwd.clone())
            .unwrap_or_else(|| "/".into()),
        backend_type: "claude".into(),
        agent_mode: meta
            .permission_mode
            .unwrap_or_else(|| "accept-edits".into()),
        model: meta.model.unwrap_or_else(|| "unknown".into()),
        created_at,
        last_activity_at: meta.last_activity_at.unwrap_or(created_at),
        main_chain_id: nodes.last().map_or(0, |n| n.node_id),
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: sidechain_parent.or_else(|| source.parent_session_id.clone()),
        agent_id,
        checkpoints: meta.checkpoints,
        metadata: Value::Object(metadata),
        nodes,
        prompt_history: meta.prompt_history,
    }
}

fn parse_entries(raw: &str) -> Vec<Map<String, Value>> {
    raw.split('\n')
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            // A truncated last line of a file being appended mid-write —
            // skip it.
            serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|v| v.as_object().cloned())
        })
        .collect()
}

/// Parse one Claude Code JSONL transcript into a full session IR: nodes,
/// tree links, usage, tool calls/results and prompt history included.
/// Never fails — blank and malformed lines are skipped.
pub fn from_jsonl(raw: &str, source: &ClaudeSourceInfo) -> Session {
    let entries = parse_entries(raw);
    let meta = collect_meta(&entries);
    let built = build_nodes(&entries, meta.created_at.unwrap_or_else(now_seconds));
    // Result entries carry `is_error` per call — fold it back onto the
    // issuing `ToolCall`s now that all results have been seen.
    let nodes = shared::apply_tool_call_outcomes(&built, &shared::tool_node_outcomes(&built));
    session_from(meta, source, nodes)
}

/// The list-time shape: same session meta [`from_jsonl`] computes, without
/// building nodes. Enough for `GET /api/sessions` — id, title, cwd, model,
/// activity times and the sidechain→parent link.
pub fn summarize_jsonl(raw: &str, source: &ClaudeSourceInfo) -> Session {
    session_from(collect_meta(&parse_entries(raw)), source, Vec::new())
}

/// Read a `<session>.jsonl` transcript into IR. The session id is the file's
/// basename; the fallback cwd decodes the project dir name it sits in, and a
/// file under `<uuid>/subagents/` reports that uuid as its parent session.
///
/// # Errors
/// `ConversionError` when the file does not exist or cannot be read.
pub fn from_file(
    file_path: &Path,
    id: Option<&str>,
    parent_session_id: Option<&str>,
) -> Result<Session, ConversionError> {
    if !file_path.exists() {
        return Err(ConversionError {
            message: format!("Claude Code transcript not found: {}", file_path.display()),
            cause: Value::Null,
        });
    }
    let raw = std::fs::read_to_string(file_path).map_err(|e| ConversionError {
        message: format!("Claude Code conversion failed: {e}"),
        cause: Value::Null,
    })?;
    let basename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let dir_name = |p: &Path| {
        p.parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    };
    let id = id.map_or_else(
        || basename.strip_suffix(JSONL).unwrap_or(basename).to_string(),
        str::to_string,
    );
    let parent_name = dir_name(file_path);
    let in_subagents = parent_name == "subagents";
    let parent_session_id = parent_session_id.map(str::to_string).or_else(|| {
        in_subagents.then(|| {
            file_path
                .parent()
                .and_then(Path::parent)
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string()
        })
    });
    let project_dir_name = if in_subagents {
        file_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        parent_name
    };
    Ok(from_jsonl(
        &raw,
        &ClaudeSourceInfo {
            id,
            fallback_cwd: Some(decode_project_dir(&project_dir_name)),
            parent_session_id,
        },
    ))
}

/* ------------------------------------------------------------------ */
/* writer                                                              */
/* ------------------------------------------------------------------ */

/// A field the reader stashed in `node.metadata`/`session.metadata`.
fn meta_field<'a>(meta: &'a Value, key: &str) -> Option<&'a Value> {
    meta.get(key)
}

fn meta_str(meta: &Value, key: &str) -> Option<String> {
    meta_field(meta, key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn meta_flag(meta: &Value, key: &str) -> bool {
    meta_field(meta, key) == Some(&Value::Bool(true))
}

/// The IR `usage` back onto the entry's `message.usage` shape.
fn usage_to_claude(usage: &TokenUsage) -> Value {
    let mut out = Map::new();
    out.insert("input_tokens".into(), json!(usage.input));
    out.insert("output_tokens".into(), json!(usage.output));
    if let Some(cache_read) = usage.cache_read {
        out.insert("cache_read_input_tokens".into(), json!(cache_read));
    }
    if let Some(cache_write) = usage.cache_write {
        out.insert("cache_creation_input_tokens".into(), json!(cache_write));
    }
    Value::Object(out)
}

/// An IR `Block` back onto a `message.content` array item.
fn block_to_claude(block: &Block) -> Option<Value> {
    match block {
        Block::Text { text } => Some(json!({ "type": "text", "text": text })),
        Block::Image {
            data,
            mime_type,
            uri,
        } => {
            if let Some(data) = data {
                return Some(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type.as_deref().unwrap_or("image/png"),
                        "data": data,
                    },
                }));
            }
            uri.as_ref().map(|uri| {
                json!({
                    "type": "image",
                    "source": { "type": "url", "url": uri },
                })
            })
        }
        Block::File {
            uri,
            name,
            mime_type,
            text,
            data,
            ..
        } => {
            let title = name
                .as_ref()
                .map_or(Value::Null, |name| json!({ "title": name }));
            let attach_title = |mut entry: Map<String, Value>| {
                if let Some(title) = title.as_object() {
                    for (k, v) in title {
                        entry.insert(k.clone(), v.clone());
                    }
                }
                Value::Object(entry)
            };
            if let Some(text) = text {
                let entry = json!({
                    "type": "document",
                    "source": {
                        "type": "text",
                        "media_type": mime_type.as_deref().unwrap_or("text/plain"),
                        "text": text,
                    },
                });
                return Some(attach_title(entry.as_object().cloned().unwrap_or_default()));
            }
            if let Some(data) = data {
                let entry = json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type.as_deref().unwrap_or("application/octet-stream"),
                        "data": data,
                    },
                });
                return Some(attach_title(entry.as_object().cloned().unwrap_or_default()));
            }
            uri.as_ref().map(|uri| {
                let entry = json!({
                    "type": "document",
                    "source": { "type": "url", "url": uri },
                });
                attach_title(entry.as_object().cloned().unwrap_or_default())
            })
        }
        // `audio` has no Claude transcript slot.
        Block::Audio { .. } => None,
    }
}

/// One `Session` → the append-only JSONL transcript Claude Code resumes
/// from. Every node becomes one entry carrying the common fields the reader
/// collects meta from (`sessionId`/`cwd`/`timestamp`/`uuid`/`parentUuid`),
/// so `from_jsonl(to_jsonl(s))` rebuilds the same node tree.
///
/// Role mapping: `tool` nodes write back as `user` entries holding
/// `tool_result` blocks (where they came from); `assistant` nodes emit
/// `thinking`/`redacted_thinking`/`text`/`tool_use` content — sealed
/// thinking only, unsigned blocks are dropped like the Devin/Cline writers.
/// Sub-agent sessions (`parent_session_id`) mark every entry `isSidechain`
/// with `sessionId` naming the parent, the layout contract of
/// `<uuid>/subagents/*.jsonl` files.
///
/// Sepia extension over the stock writer: `tool_use` items additionally
/// persist the recorded IR `locations`/`diffs` when present so a
/// save→read round-trip
/// keeps revertable file changes even for tool names `toolFileRefs` does not
/// derive from (`edit`, Cline's `editor`, …). Real transcripts never carry
/// the fields, so reading is unchanged.
pub fn to_jsonl(session: &Session) -> String {
    let parent_id = session.parent_session_id.as_deref();
    let session_meta = &session.metadata;
    let git_branch = meta_str(session_meta, "gitBranch");
    let claude_version = meta_str(session_meta, "claudeVersion");
    let slug = meta_str(session_meta, "slug");
    let agent_id = session.agent_id.as_deref();

    // A recorded `metadata.uuid` survives the round-trip; fresh sessions mint
    // one per node so the parentUuid chain always resolves.
    let uuids: Vec<String> = session
        .nodes
        .iter()
        .map(|node| {
            meta_str(&node.metadata, "uuid").unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        })
        .collect();
    let parent_uuid_of = |node: &MessageNode| -> Value {
        node.parent_node_id
            .and_then(|parent| usize::try_from(parent).ok())
            .and_then(|parent| uuids.get(parent))
            .map_or(Value::Null, |uuid| json!(uuid))
    };

    let common = |node: &MessageNode, index: usize| -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("parentUuid".into(), parent_uuid_of(node));
        // Entries in a subagent file name the owning session, not the file.
        map.insert("sessionId".into(), json!(parent_id.unwrap_or(&session.id)));
        map.insert("timestamp".into(), json!(to_iso_millis(node.created_at)));
        map.insert("cwd".into(), json!(session.working_directory));
        if parent_id.is_some() || meta_flag(&node.metadata, "isSidechain") {
            map.insert("isSidechain".into(), Value::Bool(true));
        }
        map.insert("userType".into(), json!("external"));
        map.insert("uuid".into(), json!(uuids[index]));
        if let Some(branch) = &git_branch {
            map.insert("gitBranch".into(), json!(branch));
        }
        if let Some(version) = &claude_version {
            map.insert("version".into(), json!(version));
        }
        if let Some(slug) = &slug {
            map.insert("slug".into(), json!(slug));
        }
        if let Some(agent_id) = agent_id {
            map.insert("agentId".into(), json!(agent_id));
        }
        map
    };

    let entry_for = |node: &MessageNode, index: usize| -> Value {
        match node.role {
            Role::User => {
                let mut entry = Map::new();
                entry.insert("type".into(), json!("user"));
                entry.extend(common(node, index));
                if meta_flag(&node.metadata, "isMeta") {
                    entry.insert("isMeta".into(), Value::Bool(true));
                }
                if meta_flag(&node.metadata, "isCompactSummary") {
                    entry.insert("isCompactSummary".into(), Value::Bool(true));
                }
                if meta_flag(&node.metadata, "isVisibleInTranscriptOnly") {
                    entry.insert("isVisibleInTranscriptOnly".into(), Value::Bool(true));
                }
                let content = if node.blocks.is_empty() {
                    json!(node.content)
                } else {
                    Value::Array(node.blocks.iter().filter_map(block_to_claude).collect())
                };
                entry.insert(
                    "message".into(),
                    json!({ "role": "user", "content": content }),
                );
                Value::Object(entry)
            }
            Role::Tool => {
                let mut entry = Map::new();
                entry.insert("type".into(), json!("user"));
                entry.extend(common(node, index));
                if let Some(tool_use_result) = meta_field(&node.metadata, "toolUseResult") {
                    entry.insert("toolUseResult".into(), tool_use_result.clone());
                }
                let is_error = node
                    .tool_result
                    .as_ref()
                    .is_some_and(|r| r.status == ToolCallStatus::Error);
                entry.insert(
                    "message".into(),
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": node.tool_call_id.clone().unwrap_or_default(),
                            "content": node.content,
                            "is_error": is_error,
                        }],
                    }),
                );
                Value::Object(entry)
            }
            Role::Assistant => {
                let mut content: Vec<Value> = Vec::new();
                if let (Some(thinking), Some(signature)) =
                    (node.thinking.as_ref(), node.thinking_signature.as_ref())
                {
                    content.push(if thinking == REDACTED_THINKING {
                        json!({ "type": "redacted_thinking", "data": signature })
                    } else {
                        json!({ "type": "thinking", "thinking": thinking, "signature": signature })
                    });
                }
                if !node.content.is_empty() {
                    content.push(json!({ "type": "text", "text": node.content }));
                }
                for call in &node.tool_calls {
                    let mut item = Map::new();
                    item.insert("type".into(), json!("tool_use"));
                    item.insert("id".into(), json!(call.id));
                    item.insert("name".into(), json!(call.name));
                    item.insert(
                        "input".into(),
                        if call.arguments.is_null() {
                            json!({})
                        } else {
                            call.arguments.clone()
                        },
                    );
                    // Lossless round-trip slot for stores that record file
                    // changes the input shape cannot express.
                    if !call.locations.is_empty() {
                        item.insert(
                            "locations".into(),
                            serde_json::to_value(&call.locations).unwrap_or_default(),
                        );
                    }
                    if !call.diffs.is_empty() {
                        item.insert(
                            "diffs".into(),
                            serde_json::to_value(&call.diffs).unwrap_or_default(),
                        );
                    }
                    content.push(Value::Object(item));
                }
                let mut message = Map::new();
                message.insert(
                    "id".into(),
                    json!(
                        meta_str(&node.metadata, "messageId")
                            .unwrap_or_else(|| format!("msg_{}", uuids[index]))
                    ),
                );
                message.insert("type".into(), json!("message"));
                message.insert("role".into(), json!("assistant"));
                message.insert(
                    "model".into(),
                    json!(node.model.clone().unwrap_or_else(|| session.model.clone())),
                );
                message.insert("content".into(), Value::Array(content));
                message.insert(
                    "stop_reason".into(),
                    json!(node.finish_reason.clone().unwrap_or_else(|| {
                        if node.tool_calls.is_empty() {
                            "end_turn".into()
                        } else {
                            "tool_use".into()
                        }
                    })),
                );
                if let Some(usage) = &node.usage {
                    message.insert("usage".into(), usage_to_claude(usage));
                }
                let mut entry = Map::new();
                entry.insert("type".into(), json!("assistant"));
                entry.extend(common(node, index));
                if let Some(request_id) = &node.request_id {
                    entry.insert("requestId".into(), json!(request_id));
                }
                entry.insert("message".into(), Value::Object(message));
                Value::Object(entry)
            }
            Role::System => {
                let mut entry = Map::new();
                entry.insert("type".into(), json!("system"));
                entry.extend(common(node, index));
                entry.insert(
                    "subtype".into(),
                    json!(meta_str(&node.metadata, "subtype").unwrap_or_else(|| "init".into())),
                );
                entry.insert("content".into(), json!(node.content));
                if let Some(level) = meta_field(&node.metadata, "level") {
                    entry.insert("level".into(), level.clone());
                }
                if let Some(compact) = meta_field(&node.metadata, "compactMetadata") {
                    entry.insert("compactMetadata".into(), compact.clone());
                }
                Value::Object(entry)
            }
        }
    };

    let mut entries: Vec<Value> = session
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| entry_for(node, index))
        .collect();
    // The `summary` entry is how listings get a title — emit it whenever the
    // session names one beyond its own id, pinned to the last message uuid.
    if !session.title.is_empty() {
        if let Some(last) = uuids.last() {
            entries.insert(
                0,
                json!({ "type": "summary", "summary": session.title, "leafUuid": last }),
            );
        }
    }
    let mut out = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
