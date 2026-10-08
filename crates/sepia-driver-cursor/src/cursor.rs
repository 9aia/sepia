//! `Cursor.ts` port — pure decode/encode for Cursor's two on-disk stores.
//!
//! Cursor's agent CLI keeps chats under `~/.cursor/chats/<workspace-hash>/
//! <chat-id>/`: a content-addressed `store.db` (`blobs(id, data)` + a `meta`
//! KV table) plus `meta.json` and `prompt_history.json` sidecars. `meta['0']`
//! is hex-encoded JSON whose `latestRootBlobId` names a protobuf-ish
//! "checkpoint" blob; the checkpoint's repeated field-1 entries are the
//! 32-byte blob ids of the ordered message list. Message blobs are AI-SDK
//! JSON (`role` + `content`); all other binary blobs are UI/tool projections
//! sepia does not decode — they are counted as `opaqueBlobs` in metadata.
//!
//! A second, lossy projection lives at `~/.cursor/projects/<slug>/
//! agent-transcripts/<chat-id>/<chat-id>.jsonl` (plus `subagents/*.jsonl`):
//! one JSON message per line — `text` and `tool_use` blocks but no tool
//! results, usage, or timestamps — and `{"type":"turn_ended"}` markers.
//!
//! Reasoning is unrecoverable in both: `redacted-reasoning` blob payloads are
//! opaque and the transcript projects them as `[REDACTED]`; both map to the
//! `[redacted]` thinking marker. There is no ACP runtime.
//!
//! Writes target both stores. The canonical `store.db` path is fully
//! synthesised (`store_write_plan`): blob ids are the sha256 of their bytes,
//! the checkpoint protobuf carries the ordered field-1 message refs plus
//! the field-9 workspace URI, field-10 flag and field-22 `"cli"` tag, and
//! `meta['0']` is the hex-encoded JSON row pointing at it. The checkpoint
//! fields a real Cursor writes for bookkeeping — field-5 token stats and
//! the field-8 groups of prompt/context/step/tool-detail records — are not
//! synthesised: nothing on the resume path reads them, and inventing their
//! semantics risks a malformed DAG. `to_transcript_jsonl` still mirrors the
//! lossy projection alongside, matching Cursor's own dual write.

use std::collections::HashMap;

use sepia_core::domain::{
    MessageNode, PromptHistoryEntry, REDACTED_THINKING, Role, Session, ToolCall, ToolCallDiff,
    ToolCallLocation, ToolCallStatus, ToolResultInfo,
};
use sepia_core::shared::{self, decode_project_dir};
use serde_json::{Map, Value, json};
use sha2::Digest as _;

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key)?.as_str()
}

fn as_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|n| n.is_finite())
}

/// Strip control characters below U+0020 except tab/LF/CR.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| {
            let code = *c as u32;
            code >= 32 || code == 9 || code == 10 || code == 13
        })
        .collect()
}

fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !is_hex(text) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for i in 0..out.capacity() {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/* ------------------------------------------------------------------ */
/* meta['0'] — hex-encoded JSON                                        */
/* ------------------------------------------------------------------ */

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorStoreMeta {
    pub agent_id: Option<String>,
    pub latest_root_blob_id: Option<String>,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub is_run_everything: bool,
    /// Epoch milliseconds.
    pub created_at: Option<f64>,
    pub last_used_model: Option<String>,
}

/// `meta` rows are hex-encoded JSON; plain JSON is accepted too.
pub fn parse_store_meta(value: &str) -> Option<CursorStoreMeta> {
    if value.is_empty() {
        return None;
    }
    let decoded = from_hex(value);
    let decoded_text = decoded.as_deref().map(utf8);
    let candidates: [&str; 2] = [value, decoded_text.as_deref().unwrap_or("")];
    for (i, candidate) in candidates.iter().enumerate() {
        if i == 1 && decoded_text.is_none() {
            break;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(candidate) else {
            continue;
        };
        let Value::Object(obj) = parsed else {
            continue;
        };
        return Some(CursorStoreMeta {
            agent_id: str_field(&obj, "agentId").map(str::to_string),
            latest_root_blob_id: str_field(&obj, "latestRootBlobId").map(str::to_string),
            name: str_field(&obj, "name").map(str::to_string),
            mode: str_field(&obj, "mode").map(str::to_string),
            is_run_everything: obj.get("isRunEverything").and_then(Value::as_bool) == Some(true),
            created_at: finite_number(obj.get("createdAt")),
            last_used_model: str_field(&obj, "lastUsedModel").map(str::to_string),
        });
    }
    None
}

/* ------------------------------------------------------------------ */
/* meta.json sidecar                                                   */
/* ------------------------------------------------------------------ */

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorMetaJson {
    pub schema_version: Option<f64>,
    pub created_at_ms: Option<f64>,
    pub updated_at_ms: Option<f64>,
    pub title: Option<String>,
    pub has_conversation: bool,
    pub cwd: Option<String>,
}

/// Parse a `meta.json` payload (already decoded to a `Value`).
pub fn parse_meta_json(parsed: &Value) -> Option<CursorMetaJson> {
    let Value::Object(obj) = parsed else {
        return None;
    };
    Some(CursorMetaJson {
        schema_version: finite_number(obj.get("schemaVersion")),
        created_at_ms: finite_number(obj.get("createdAtMs")),
        updated_at_ms: finite_number(obj.get("updatedAtMs")),
        title: str_field(obj, "title").map(str::to_string),
        has_conversation: obj.get("hasConversation").and_then(Value::as_bool) == Some(true),
        cwd: str_field(obj, "cwd").map(str::to_string),
    })
}

/// `prompt_history.json` is a flat JSON array of submitted prompt strings.
pub fn parse_prompt_history(raw: &str) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    match parsed {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/* ------------------------------------------------------------------ */
/* Protobuf-ish checkpoint blobs                                        */
/* ------------------------------------------------------------------ */

struct ProtoField {
    field: u64,
    wire: u64,
    #[allow(dead_code)]
    varint: u64,
    data: Option<Vec<u8>>,
}

/// Returns `(value, next_pos)`; `None` when the varint never terminates.
fn read_varint(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    let mut i = pos;
    while i < buf.len() && shift < 64 {
        let b = buf[i];
        i += 1;
        // Bits shifted past 64 drop off — the same lossy behavior as the
        // TS `result += (b & 0x7f) * 2 ** shift` beyond float precision.
        result |= u64::from(b & 0x7f).wrapping_shl(shift);
        if b & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
    }
    None
}

/// Lenient protobuf field walk; `None` when the buffer isn't field-shaped.
fn parse_proto_fields(buf: &[u8]) -> Option<Vec<ProtoField>> {
    let mut fields: Vec<ProtoField> = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let (key, next) = read_varint(buf, pos)?;
        pos = next;
        let field = key >> 3;
        let wire = key & 7;
        if field == 0 {
            return None;
        }
        match wire {
            0 => {
                let (v, next) = read_varint(buf, pos)?;
                pos = next;
                fields.push(ProtoField {
                    field,
                    wire,
                    varint: v,
                    data: None,
                });
            }
            2 => {
                let (len, next) = read_varint(buf, pos)?;
                pos = next;
                let len = usize::try_from(len).unwrap_or(usize::MAX);
                if len > buf.len() - pos {
                    return None;
                }
                fields.push(ProtoField {
                    field,
                    wire,
                    varint: 0,
                    data: Some(buf[pos..pos + len].to_vec()),
                });
                pos += len;
            }
            5 => {
                if pos + 4 > buf.len() {
                    return None;
                }
                fields.push(ProtoField {
                    field,
                    wire,
                    varint: 0,
                    data: None,
                });
                pos += 4;
            }
            1 => {
                if pos + 8 > buf.len() {
                    return None;
                }
                fields.push(ProtoField {
                    field,
                    wire,
                    varint: 0,
                    data: None,
                });
                pos += 8;
            }
            _ => return None,
        }
    }
    Some(fields)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckpointInfo {
    /// Ordered message-blob ids — the transcript prefix at this checkpoint.
    pub message_ids: Vec<String>,
    /// `file://` workspace URI the chat ran in.
    pub workspace: Option<String>,
    /// Client kind, e.g. `"cli"`.
    pub client: Option<String>,
}

/**
 * A checkpoint blob: repeated field-1 length-32 entries are the content
 * hashes of the ordered messages; field 9 is the workspace URI and field 22
 * the client tag. Other fields (summary refs, UI state, counters) are not
 * decoded.
 */
pub fn decode_checkpoint(data: &[u8]) -> Option<CheckpointInfo> {
    let fields = parse_proto_fields(data)?;
    let mut message_ids: Vec<String> = Vec::new();
    let mut workspace: Option<String> = None;
    let mut client: Option<String> = None;
    for f in &fields {
        if f.wire != 2 {
            continue;
        }
        let Some(data) = &f.data else { continue };
        match f.field {
            1 if data.len() == 32 => message_ids.push(to_hex(data)),
            9 => workspace = Some(utf8(data)),
            22 => client = Some(utf8(data)),
            _ => {}
        }
    }
    if message_ids.is_empty() && workspace.is_none() && client.is_none() {
        return None;
    }
    Some(CheckpointInfo {
        message_ids,
        workspace,
        client,
    })
}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// `decodeURIComponent` — `%XX` sequences decode to UTF-8; `+` stays literal;
/// malformed input returns `None` (the JS call throws, caught as `undefined`).
fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            out.push(hex_val(bytes[i + 1])? << 4 | hex_val(bytes[i + 2])?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `file:///home/me/proj` → `/home/me/proj`; anything else is dropped.
pub fn workspace_from_uri(uri: Option<&str>) -> Option<String> {
    let uri = uri?;
    let rest = uri.strip_prefix("file://")?;
    let path = decode_uri_component(rest)?;
    if path.is_empty() { None } else { Some(path) }
}

/// `encodeURIComponent` — every byte outside the JS unreserved set is
/// percent-encoded (uppercase hex).
fn encode_uri_component(text: &str) -> String {
    const UNRESERVED: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()";
    let mut out = String::new();
    for b in text.as_bytes() {
        if UNRESERVED.contains(b) {
            out.push(*b as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/* ------------------------------------------------------------------ */
/* store.db → IR                                                       */
/* ------------------------------------------------------------------ */

/// What a chat dir proves about the session before blobs are decoded.
#[derive(Clone, Debug, Default)]
pub struct CursorChatInfo {
    /// Chat id — the directory name under `chats/<workspace-hash>/`.
    pub id: String,
    /// The opaque workspace-hash dir the chat sits under (not decodable).
    pub workspace_hash: Option<String>,
    /// Working directory when nothing in the store records one.
    pub fallback_cwd: Option<String>,
}

/// Everything `session_from_store` needs, already read off disk/sqlite.
#[derive(Clone, Debug, Default)]
pub struct CursorStoreInput {
    pub id: String,
    pub workspace_hash: Option<String>,
    pub fallback_cwd: Option<String>,
    pub meta: Option<CursorStoreMeta>,
    pub meta_json: Option<CursorMetaJson>,
    /// `blobs` rows in query order — the checkpoint fallback scan is
    /// order-sensitive on ties.
    pub blobs: Vec<(String, Vec<u8>)>,
    pub prompt_history: Vec<String>,
}

struct BlobMessage {
    role: String,
    content: Value,
    provider_options: Option<Map<String, Value>>,
}

fn parse_message(data: &[u8]) -> Option<BlobMessage> {
    let parsed: Value = serde_json::from_slice(data).ok()?;
    let Value::Object(obj) = parsed else {
        return None;
    };
    let role = obj.get("role")?.as_str()?.to_string();
    Some(BlobMessage {
        role,
        content: obj.get("content").cloned().unwrap_or(Value::Null),
        provider_options: obj
            .get("providerOptions")
            .and_then(Value::as_object)
            .cloned(),
    })
}

fn cursor_options(msg: &BlobMessage) -> Option<&Map<String, Value>> {
    msg.provider_options.as_ref()?.get("cursor")?.as_object()
}

fn strip_angle_tags(text: &str) -> String {
    // `<[^>]+>` → " ": a `<` with no following `>` survives verbatim.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        if let Some(gt) = rest[lt..].find('>') {
            out.push(' ');
            rest = &rest[lt + gt + 1..];
        } else {
            out.push_str(&rest[lt..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Inner text of a `<user_query>…</user_query>` block, or the text itself.
pub fn extract_user_query(text: &str) -> Option<String> {
    let inner = text.find("<user_query>").and_then(|start| {
        let rest = &text[start + "<user_query>".len()..];
        rest.find("</user_query>").map(|end| &rest[..end])
    });
    let raw = match inner {
        Some(inner) => inner.to_string(),
        None => strip_angle_tags(text),
    };
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn is_user_info(content: &Value) -> bool {
    content
        .as_str()
        .is_some_and(|s| s.trim_start().starts_with("<user_info>"))
}

/**
 * `providerOptions.cursor.highLevelToolCallResult.output` carries the real
 * outcome — `isError` plus per-tool payloads (`success.executionTime` for
 * Shell). Non-Shell shapes keep whatever fields exist.
 */
fn tool_result_info(result: &Map<String, Value>, msg: &BlobMessage) -> ToolResultInfo {
    let output = cursor_options(msg)
        .and_then(|cursor| cursor.get("highLevelToolCallResult"))
        .and_then(|high| high.get("output"))
        .and_then(Value::as_object);
    let is_error = output
        .and_then(|o| o.get("isError"))
        .and_then(Value::as_bool)
        == Some(true)
        || result.get("is_error").and_then(Value::as_bool) == Some(true);
    let success = output
        .and_then(|o| o.get("success"))
        .and_then(Value::as_object);
    let duration_ms = success
        .and_then(|s| finite_number(s.get("executionTime")))
        .or_else(|| output.and_then(|o| finite_number(o.get("executionTime"))));
    ToolResultInfo {
        status: if is_error {
            ToolCallStatus::Error
        } else {
            ToolCallStatus::Success
        },
        exit_code: None,
        duration_ms,
    }
}

fn tool_result_text(result: Option<&Value>) -> String {
    match result {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[derive(Default)]
struct ToolPairing {
    name_by_id: HashMap<String, String>,
    args_by_id: HashMap<String, Value>,
}

/* ------------------------------------------------------------------ */
/* Tool inputs → locations/diffs (shared by store + transcript)         */
/* ------------------------------------------------------------------ */

/// The file refs a tool call's args record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolFileRefs {
    pub locations: Vec<ToolCallLocation>,
    pub diffs: Vec<ToolCallDiff>,
}

const PATCH_DIRECTIVES: [&str; 6] = [
    "Add File",
    "Update File",
    "Delete File",
    "Move to",
    "End Patch",
    "Begin Patch",
];

/// `^\*\*\*\s*(directive)\s*(?::\s*(.*))?$` — a `*** <Verb>` op line.
fn patch_op(line: &str) -> Option<(&'static str, Option<&str>)> {
    let rest = line.strip_prefix("***")?;
    let rest = rest.trim_start();
    for directive in PATCH_DIRECTIVES {
        let Some(after) = rest.strip_prefix(directive) else {
            continue;
        };
        let trimmed = after.trim_start();
        if trimmed.is_empty() {
            return Some((directive, None));
        }
        return trimmed
            .strip_prefix(':')
            .map(|operand| (directive, Some(operand.trim_start())));
    }
    None
}

/**
 * One `*** <Verb> File:` section of an `ApplyPatch` payload. V4A patches are
 * the only Cursor tool input that is a raw string rather than an object —
 * hunks (`@@` … context/`-`/`+` lines) still map onto the same
 * `{oldText, newText}` diff shape a restore reverse-applies.
 */
fn apply_patch_refs(patch: &str) -> ToolFileRefs {
    #[derive(Clone, Copy, PartialEq)]
    enum Verb {
        Add,
        Update,
        Delete,
    }

    let mut locations: Vec<ToolCallLocation> = Vec::new();
    let mut diffs: Vec<ToolCallDiff> = Vec::new();
    // Path of the section currently accumulating hunks; `Move to` redirects
    // it — the rename itself isn't a diff, but the edit lands on the new name.
    let mut path: Option<String> = None;
    let mut verb: Option<Verb> = None;
    let mut hunk: Option<(Vec<String>, Vec<String>)> = None;

    macro_rules! flush_hunk {
        () => {
            // A hunk that recorded no payload lines still leaves a bare
            // `{path}` entry — the change is on record even when nothing is
            // revertable.
            if let (Some((old_lines, new_lines)), Some(path)) = (hunk.take(), path.as_ref()) {
                let old_text = old_lines.join("\n");
                let new_text = new_lines.join("\n");
                diffs.push(ToolCallDiff {
                    path: path.clone(),
                    old_text: (!old_text.is_empty()).then_some(old_text),
                    new_text: (!new_text.is_empty()).then_some(new_text),
                });
            }
        };
    }
    // `flushSection` in the TS = flush_hunk + clear the verb. `start_section`
    // only needs the hunk flush — it overwrites `verb` right after.
    macro_rules! flush_section {
        () => {{
            flush_hunk!();
            verb = None;
        }};
    }
    macro_rules! start_section {
        ($next_verb:expr, $next_path:expr) => {{
            flush_hunk!();
            verb = Some($next_verb);
            path = Some($next_path.to_string());
            locations.push(ToolCallLocation {
                path: $next_path.to_string(),
                line: None,
            });
            if matches!($next_verb, Verb::Add | Verb::Delete) {
                hunk = Some((Vec::new(), Vec::new()));
            }
        }};
    }

    for raw in patch.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some((directive, operand)) = patch_op(line) {
            let operand = operand.unwrap_or("");
            match directive {
                "Add File" => start_section!(Verb::Add, operand),
                "Update File" => start_section!(Verb::Update, operand),
                "Delete File" => start_section!(Verb::Delete, operand),
                "Move to" => {
                    // The file was renamed then edited — hunks land on the
                    // new name; the old name keeps only its location entry.
                    if !operand.is_empty() {
                        path = Some(operand.to_string());
                        locations.push(ToolCallLocation {
                            path: operand.to_string(),
                            line: None,
                        });
                    }
                }
                "End Patch" => flush_section!(),
                _ => {}
            }
            continue;
        }
        if line.starts_with("@@") {
            // New hunk inside an update section (the marker's trailing
            // context is a locate hint, not part of the recorded change).
            flush_hunk!();
            if verb == Some(Verb::Update) {
                hunk = Some((Vec::new(), Vec::new()));
            }
            continue;
        }
        let Some((old_lines, new_lines)) = hunk.as_mut() else {
            continue;
        };
        if line == "\\ No newline at end of file" {
            continue;
        }
        match line.chars().next() {
            // Context lines belong to both sides.
            Some(' ') => {
                let body = &line[1..];
                old_lines.push(body.to_string());
                new_lines.push(body.to_string());
            }
            Some('-') => old_lines.push(line[1..].to_string()),
            Some('+') => new_lines.push(line[1..].to_string()),
            _ => {}
        }
    }
    // Trailing `flushSection` — the verb clear is dead past the loop.
    flush_hunk!();
    ToolFileRefs { locations, diffs }
}

/**
 * File paths a Cursor tool call's args name — `StrReplace`/`Write` inputs
 * carry the before/after payloads a restore reverse-applies (the same
 * `{old,new}_string` contract Cline's `editor` uses), `Delete` records only
 * the path it removed, and `ApplyPatch`'s raw patch string decodes through
 * `apply_patch_refs`. Read-style tools contribute locations.
 */
pub fn tool_file_refs(name: &str, args: &Value) -> ToolFileRefs {
    if name == "ApplyPatch" {
        // The args are the patch itself — a bare string, or an object
        // holding it under a patch-ish key.
        let text = if let Some(s) = args.as_str() {
            Some(s.to_string())
        } else {
            args.as_object().and_then(|obj| {
                ["patch", "input", "content", "diff"]
                    .iter()
                    .find_map(|key| str_field(obj, key))
                    .map(str::to_string)
            })
        };
        return match text {
            Some(text) => apply_patch_refs(&text),
            None => ToolFileRefs::default(),
        };
    }
    let Some(obj) = args.as_object() else {
        return ToolFileRefs::default();
    };
    let path = str_field(obj, "path").or_else(|| str_field(obj, "target_notebook"));
    let mut locations: Vec<ToolCallLocation> = Vec::new();
    if let Some(path) = path {
        locations.push(ToolCallLocation {
            path: path.to_string(),
            line: None,
        });
    }
    for key in ["paths", "target_directories"] {
        if let Some(list) = obj.get(key).and_then(Value::as_array) {
            for item in list {
                if let Some(s) = item.as_str().filter(|s| !s.is_empty()) {
                    locations.push(ToolCallLocation {
                        path: s.to_string(),
                        line: None,
                    });
                }
            }
        }
    }
    if let Some(dir) = str_field(obj, "target_directory") {
        locations.push(ToolCallLocation {
            path: dir.to_string(),
            line: None,
        });
    }

    match name {
        "StrReplace" | "Edit" | "EditNotebook" => {
            let Some(path) = path else {
                return ToolFileRefs {
                    locations,
                    diffs: Vec::new(),
                };
            };
            let old_text = str_field(obj, "old_string");
            let new_text = str_field(obj, "new_string");
            let diffs = if old_text.is_none() && new_text.is_none() {
                Vec::new()
            } else {
                vec![ToolCallDiff {
                    path: path.to_string(),
                    old_text: old_text.map(str::to_string),
                    new_text: new_text.map(str::to_string),
                }]
            };
            ToolFileRefs { locations, diffs }
        }
        "Write" => {
            let content = str_field(obj, "contents").or_else(|| str_field(obj, "content"));
            let diffs = match (path, content) {
                (Some(path), Some(content)) => vec![ToolCallDiff {
                    path: path.to_string(),
                    old_text: None,
                    new_text: Some(content.to_string()),
                }],
                _ => Vec::new(),
            };
            ToolFileRefs { locations, diffs }
        }
        _ => ToolFileRefs {
            locations,
            diffs: Vec::new(),
        },
    }
}

fn base_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    role: Role,
    content: String,
    created_at: f64,
    metadata: Value,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
        role,
        content,
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
        metadata,
    }
}

/// One decoded message blob → the IR node(s) it emits.
fn message_nodes(
    msg: &BlobMessage,
    blob_id: &str,
    node_id: i64,
    parent: Option<i64>,
    ts: f64,
    pairing: &mut ToolPairing,
) -> Vec<MessageNode> {
    if msg.role == "system" {
        let content = match &msg.content {
            Value::String(s) => s.clone(),
            other => tool_result_text(Some(other)),
        };
        return vec![base_node(
            node_id,
            parent,
            Role::System,
            sanitize(&content),
            ts,
            json!({ "blobId": blob_id }),
        )];
    }

    if msg.role == "user" {
        if let Value::String(text) = &msg.content {
            let metadata = if is_user_info(&msg.content) {
                json!({ "blobId": blob_id, "context": "user_info" })
            } else {
                json!({ "blobId": blob_id })
            };
            return vec![base_node(
                node_id,
                parent,
                Role::User,
                sanitize(text),
                ts,
                metadata,
            )];
        }
        let Some(items) = msg.content.as_array() else {
            return Vec::new();
        };
        let text_parts: Vec<&str> = items
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                if obj.get("type").and_then(Value::as_str) == Some("text") {
                    obj.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect();
        if text_parts.is_empty() {
            return Vec::new();
        }
        let request_id = cursor_options(msg)
            .and_then(|cursor| str_field(cursor, "requestId"))
            .map(str::to_string);
        let mut node = base_node(
            node_id,
            parent,
            Role::User,
            sanitize(&text_parts.join("\n")),
            ts,
            json!({ "blobId": blob_id }),
        );
        node.request_id = request_id;
        return vec![node];
    }

    if msg.role == "assistant" {
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut redacted = false;
        // The `redacted-reasoning` payload is opaque; it rides the IR
        // verbatim as the thinking block's signature so a converted session
        // keeps the seal.
        let mut redacted_data: Option<String> = None;
        let items: &[Value] = msg.content.as_array().map_or(&[], Vec::as_slice);
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            match obj.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = obj.get("text").and_then(Value::as_str) {
                        text_parts.push(text.to_string());
                    }
                }
                Some("redacted-reasoning") => {
                    redacted = true;
                    if let Some(data) = str_field(obj, "data") {
                        redacted_data = Some(data.to_string());
                    }
                }
                Some("tool-call") => {
                    let id = str_field(obj, "toolCallId").map_or_else(
                        || format!("cursor-tool-{node_id}-{}", tool_calls.len()),
                        str::to_string,
                    );
                    let name = str_field(obj, "toolName").unwrap_or("unknown").to_string();
                    // `args ?? input ?? {}` — nulls fall through.
                    let args = ["args", "input"]
                        .iter()
                        .filter_map(|key| obj.get(*key))
                        .find(|v| !v.is_null())
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let refs = tool_file_refs(&name, &args);
                    pairing.name_by_id.insert(id.clone(), name.clone());
                    pairing.args_by_id.insert(id.clone(), args.clone());
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: args,
                        index: i64::try_from(tool_calls.len()).unwrap_or(i64::MAX),
                        kind: "function".into(),
                        status: None,
                        exit_code: None,
                        duration_ms: None,
                        locations: refs.locations,
                        diffs: refs.diffs,
                    });
                }
                _ => {}
            }
        }
        let mut node = base_node(
            node_id,
            parent,
            Role::Assistant,
            sanitize(&text_parts.join("\n")),
            ts,
            if redacted {
                json!({ "blobId": blob_id, "redactedReasoning": true })
            } else {
                json!({ "blobId": blob_id })
            },
        );
        if redacted {
            node.thinking = Some(REDACTED_THINKING.to_string());
        }
        node.thinking_signature = redacted_data;
        node.tool_calls = tool_calls;
        return vec![node];
    }

    if msg.role == "tool" {
        let items: &[Value] = msg.content.as_array().map_or(&[], Vec::as_slice);
        let mut nodes: Vec<MessageNode> = Vec::new();
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            if obj.get("type").and_then(Value::as_str) != Some("tool-result") {
                continue;
            }
            let call_id = str_field(obj, "toolCallId");
            let result_name = str_field(obj, "toolName");
            let tool_name = result_name
                .map(str::to_string)
                .or_else(|| call_id.and_then(|id| pairing.name_by_id.get(id)).cloned());
            let mut node = base_node(
                node_id + i64::try_from(nodes.len()).unwrap_or(i64::MAX),
                parent,
                Role::Tool,
                sanitize(&tool_result_text(obj.get("result"))),
                ts,
                json!({
                    "blobId": blob_id,
                    "toolArguments": call_id
                        .and_then(|id| pairing.args_by_id.get(id))
                        .cloned()
                        .unwrap_or(Value::Null),
                }),
            );
            node.tool_call_id = call_id.map(str::to_string);
            node.tool_name = tool_name;
            node.tool_result = Some(tool_result_info(obj, msg));
            nodes.push(node);
        }
        if nodes.is_empty() {
            nodes.push(base_node(
                node_id,
                parent,
                Role::Tool,
                sanitize(&tool_result_text(Some(&msg.content))),
                ts,
                json!({ "blobId": blob_id }),
            ));
        }
        return nodes;
    }

    vec![base_node(
        node_id,
        parent,
        Role::System,
        sanitize(&format!("[cursor {}]", msg.role)),
        ts,
        json!({ "blobId": blob_id, "role": msg.role }),
    )]
}

fn title_from(title: Option<&str>, first_user: Option<&str>, id: &str) -> String {
    match title {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => first_user.map_or_else(|| id.to_string(), |s| s.chars().take(80).collect()),
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Blank `Session` carrying the cursor defaults every projection shares.
fn session_shell(id: &str, title: String, working_directory: String) -> Session {
    Session {
        id: id.to_string(),
        title,
        working_directory,
        backend_type: "cursor".into(),
        agent_mode: "default".into(),
        model: "unknown".into(),
        created_at: 0.0,
        last_activity_at: 0.0,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: Value::Null,
        nodes: Vec::new(),
        prompt_history: Vec::new(),
    }
}

/**
 * Full IR for a `chats/<ws>/<chat>/` store: the latest checkpoint's ordered
 * message list decoded into nodes. Binary blobs that are not message JSON
 * (checkpoints, prompt records, UI projections) are skipped and counted.
 */
pub fn session_from_store(input: &CursorStoreInput) -> Session {
    let created_ms = input
        .meta_json
        .as_ref()
        .and_then(|m| m.created_at_ms)
        .or_else(|| input.meta.as_ref().and_then(|m| m.created_at))
        .unwrap_or_else(now_ms);
    let ts = (created_ms / 1000.0).floor();

    let blobs_by_id: HashMap<&str, &[u8]> = input
        .blobs
        .iter()
        .map(|(id, data)| (id.as_str(), data.as_slice()))
        .collect();
    let root_id = input
        .meta
        .as_ref()
        .and_then(|m| m.latest_root_blob_id.as_deref());
    let root_blob = root_id.and_then(|id| blobs_by_id.get(id).copied());
    let mut checkpoint = root_blob.and_then(|blob| {
        (!blob.is_empty())
            .then(|| decode_checkpoint(blob))
            .flatten()
    });
    if checkpoint.is_none() {
        // No usable `latestRootBlobId` (missing/corrupt meta row): fall back
        // to the blob that decodes as the richest checkpoint.
        for (_, data) in &input.blobs {
            if data.is_empty() {
                continue;
            }
            if let Some(candidate) = decode_checkpoint(data) {
                if candidate.message_ids.len()
                    > checkpoint.as_ref().map_or(0, |c| c.message_ids.len())
                {
                    checkpoint = Some(candidate);
                }
            }
        }
    }
    let message_ids = checkpoint
        .as_ref()
        .map_or_else(Vec::new, |c| c.message_ids.clone());
    let workspace = workspace_from_uri(checkpoint.as_ref().and_then(|c| c.workspace.as_deref()));
    let client = checkpoint.as_ref().and_then(|c| c.client.clone());

    let mut nodes: Vec<MessageNode> = Vec::new();
    let mut pairing = ToolPairing::default();
    let mut first_user_text: Option<String> = None;
    let mut opaque_blobs = 0i64;
    let mut last_node_id: Option<i64> = None;

    for id in &message_ids {
        let Some(data) = blobs_by_id.get(id.as_str()).copied() else {
            opaque_blobs += 1;
            continue;
        };
        if data.is_empty() {
            opaque_blobs += 1;
            continue;
        }
        let Some(msg) = parse_message(data) else {
            opaque_blobs += 1;
            continue;
        };
        let emitted = message_nodes(
            &msg,
            id,
            as_i64(nodes.len()),
            last_node_id,
            ts,
            &mut pairing,
        );
        for node in emitted {
            // `<user_info>` context is environment plumbing, not a real prompt.
            let is_context =
                node.metadata.get("context").and_then(Value::as_str) == Some("user_info");
            if node.role == Role::User && !is_context && first_user_text.is_none() {
                first_user_text = extract_user_query(&node.content);
            }
            last_node_id = Some(node.node_id);
            nodes.push(node);
        }
    }

    let folded = shared::apply_tool_call_outcomes(&nodes, &shared::tool_node_outcomes(&nodes));

    let prompt_history: Vec<PromptHistoryEntry> = input
        .prompt_history
        .iter()
        .map(|content| PromptHistoryEntry {
            content: sanitize(content),
            timestamp: created_ms,
            is_shell: false,
        })
        .collect();

    let updated_ms = input
        .meta_json
        .as_ref()
        .and_then(|m| m.updated_at_ms)
        .unwrap_or(created_ms);
    let mut session = session_shell(
        &input.id,
        title_from(
            input
                .meta_json
                .as_ref()
                .and_then(|m| m.title.as_deref())
                .or_else(|| input.meta.as_ref().and_then(|m| m.name.as_deref())),
            first_user_text.as_deref(),
            &input.id,
        ),
        input
            .meta_json
            .as_ref()
            .and_then(|m| m.cwd.clone())
            .or(workspace)
            .or_else(|| input.fallback_cwd.clone())
            .unwrap_or_else(|| "/".into()),
    );
    session.agent_mode = input
        .meta
        .as_ref()
        .and_then(|m| m.mode.clone())
        .unwrap_or_else(|| "default".into());
    session.model = input
        .meta
        .as_ref()
        .and_then(|m| m.last_used_model.clone())
        .unwrap_or_else(|| "unknown".into());
    session.created_at = ts;
    session.last_activity_at = ts.max((updated_ms / 1000.0).floor());
    session.main_chain_id = folded.last().map_or(0, |n| n.node_id);
    session.metadata = json!({
        "source": "cursor",
        "store": "chats",
        "workspaceHash": input.workspace_hash,
        "agentId": input.meta.as_ref().and_then(|m| m.agent_id.clone()),
        "mode": input.meta.as_ref().and_then(|m| m.mode.clone()),
        "isRunEverything": input.meta.as_ref().map(|m| m.is_run_everything),
        "latestRootBlobId": root_id,
        "client": client,
        "blobCount": as_i64(blobs_by_id.len()),
        "opaqueBlobs": opaque_blobs,
    });
    session.nodes = folded;
    session.prompt_history = prompt_history;
    session
}

/// Extra fields `summarize_store` accepts beyond [`CursorChatInfo`].
#[derive(Clone, Debug, Default)]
pub struct SummarizeStoreInput {
    pub chat: CursorChatInfo,
    pub meta: Option<CursorStoreMeta>,
    pub meta_json: Option<CursorMetaJson>,
    pub workspace: Option<String>,
    pub mtime_ms: Option<f64>,
}

/**
 * The list-time shape for a chat dir — meta and workspace without decoding
 * the message list. `workspace` is the checkpoint's field-9 path when the
 * repository already opened the store; meta.json `cwd` wins when present.
 */
pub fn summarize_store(input: &SummarizeStoreInput) -> Session {
    let created_ms = input
        .meta_json
        .as_ref()
        .and_then(|m| m.created_at_ms)
        .or_else(|| input.meta.as_ref().and_then(|m| m.created_at))
        .or(input.mtime_ms)
        .unwrap_or_else(now_ms);
    let updated_ms = input
        .meta_json
        .as_ref()
        .and_then(|m| m.updated_at_ms)
        .or(input.mtime_ms)
        .unwrap_or(created_ms);
    let named = input
        .meta_json
        .as_ref()
        .and_then(|m| m.title.as_deref())
        .or_else(|| input.meta.as_ref().and_then(|m| m.name.as_deref()));
    let mut session = session_shell(
        &input.chat.id,
        match named {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => input.chat.id.clone(),
        },
        input
            .meta_json
            .as_ref()
            .and_then(|m| m.cwd.clone())
            .or_else(|| input.workspace.clone())
            .or_else(|| input.chat.fallback_cwd.clone())
            .unwrap_or_else(|| "/".into()),
    );
    session.agent_mode = input
        .meta
        .as_ref()
        .and_then(|m| m.mode.clone())
        .unwrap_or_else(|| "default".into());
    session.model = input
        .meta
        .as_ref()
        .and_then(|m| m.last_used_model.clone())
        .unwrap_or_else(|| "unknown".into());
    session.created_at = (created_ms / 1000.0).floor();
    session.last_activity_at = (created_ms.max(updated_ms) / 1000.0).floor();
    session.metadata = json!({
        "source": "cursor",
        "store": "chats",
        "workspaceHash": input.chat.workspace_hash,
        "agentId": input.meta.as_ref().and_then(|m| m.agent_id.clone()),
        "latestRootBlobId": input.meta.as_ref().and_then(|m| m.latest_root_blob_id.clone()),
    });
    session
}

// ------------------------------------------------------------------
// agent-transcripts/*.jsonl → IR (lossy projection)
// ------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct CursorTranscriptSource {
    /// Chat id — the transcript dir/file name.
    pub id: String,
    /// Project dir name; decodes to the working directory when no cwd exists.
    pub project_slug: String,
    /// Set when the file lives under `<chat>/subagents/` — the parent chat id.
    pub parent_session_id: Option<String>,
    /// File mtime in ms — the only timestamp the projection has.
    pub mtime_ms: Option<f64>,
}

fn parse_lines(raw: &str) -> Vec<Map<String, Value>> {
    raw.split('\n')
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|v| v.as_object().cloned())
        })
        .collect()
}

/// Text content of a transcript `message.content` — the text parts joined,
/// or the string content verbatim; empty when neither shape exists.
fn transcript_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                if obj.get("type").and_then(Value::as_str) == Some("text") {
                    obj.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

struct TranscriptMeta {
    first_user_text: Option<String>,
    prompt_history: Vec<PromptHistoryEntry>,
    turn_errors: Vec<Value>,
}

fn transcript_meta(lines: &[Map<String, Value>], ts_ms: f64) -> TranscriptMeta {
    let mut first_user_text: Option<String> = None;
    let mut prompt_history: Vec<PromptHistoryEntry> = Vec::new();
    let mut turn_errors: Vec<Value> = Vec::new();
    for line in lines {
        if line.get("type").and_then(Value::as_str) == Some("turn_ended") {
            if line.get("status").and_then(Value::as_str) == Some("error") {
                turn_errors.push(line.get("error").cloned().unwrap_or(Value::Null));
            }
            continue;
        }
        if line.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let content = line
            .get("message")
            .and_then(Value::as_object)
            .and_then(|m| m.get("content"));
        let text = transcript_text(content);
        let Some(query) = extract_user_query(&text) else {
            continue;
        };
        if first_user_text.is_none() {
            first_user_text = Some(query.clone());
        }
        prompt_history.push(PromptHistoryEntry {
            content: sanitize(&query),
            timestamp: ts_ms,
            is_shell: false,
        });
    }
    TranscriptMeta {
        first_user_text,
        prompt_history,
        turn_errors,
    }
}

/// `[REDACTED]` inside a text block is the transcript's projection of a
/// redacted reasoning block — strip it and mark thinking.
fn strip_redacted(text: &str) -> (String, bool) {
    if !text.contains("[REDACTED]") {
        return (text.to_string(), false);
    }
    // `\s*\[REDACTED\]` — whitespace before the marker goes too.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[REDACTED]") {
        out.push_str(rest[..at].trim_end());
        rest = &rest[at + "[REDACTED]".len()..];
    }
    out.push_str(rest);
    (out, true)
}

fn transcript_nodes(lines: &[Map<String, Value>], ts: f64) -> Vec<MessageNode> {
    let mut nodes: Vec<MessageNode> = Vec::new();
    let mut last_node_id: Option<i64> = None;

    for line in lines {
        let message = line.get("message").and_then(Value::as_object);
        let content = message.and_then(|m| m.get("content"));
        let blocks: &[Value] = content.and_then(Value::as_array).map_or(&[], Vec::as_slice);

        match line.get("role").and_then(Value::as_str) {
            Some("user") => {
                let text = transcript_text(content);
                if text.is_empty() {
                    continue;
                }
                let node_id = as_i64(nodes.len());
                nodes.push(base_node(
                    node_id,
                    last_node_id,
                    Role::User,
                    sanitize(&text),
                    ts,
                    json!({ "projection": "transcript" }),
                ));
                last_node_id = Some(node_id);
            }
            Some("assistant") => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<ToolCall> = Vec::new();
                let mut redacted = false;
                for item in blocks {
                    let Some(obj) = item.as_object() else {
                        continue;
                    };
                    match obj.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = obj.get("text").and_then(Value::as_str) {
                                let (cleaned, hit) = strip_redacted(text);
                                if hit {
                                    redacted = true;
                                }
                                if !cleaned.is_empty() {
                                    text_parts.push(cleaned);
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = str_field(obj, "name").unwrap_or("unknown").to_string();
                            // `input === undefined ? {} : input` — a null
                            // input stays null here (unlike the blob path).
                            let args = obj.get("input").cloned().unwrap_or_else(|| json!({}));
                            let refs = tool_file_refs(&name, &args);
                            let id = str_field(obj, "id").map_or_else(
                                || format!("cursor-tool-{}-{}", nodes.len(), tool_calls.len()),
                                str::to_string,
                            );
                            tool_calls.push(ToolCall {
                                id,
                                name,
                                arguments: args,
                                index: i64::try_from(tool_calls.len()).unwrap_or(i64::MAX),
                                kind: "function".into(),
                                status: None,
                                exit_code: None,
                                duration_ms: None,
                                locations: refs.locations,
                                diffs: refs.diffs,
                            });
                        }
                        _ => {}
                    }
                }
                if text_parts.is_empty() && tool_calls.is_empty() && !redacted {
                    continue;
                }
                let node_id = as_i64(nodes.len());
                let mut node = base_node(
                    node_id,
                    last_node_id,
                    Role::Assistant,
                    sanitize(&text_parts.join("\n")),
                    ts,
                    if redacted {
                        json!({ "projection": "transcript", "redactedReasoning": true })
                    } else {
                        json!({ "projection": "transcript" })
                    },
                );
                if redacted {
                    node.thinking = Some(REDACTED_THINKING.to_string());
                }
                node.tool_calls = tool_calls;
                nodes.push(node);
                last_node_id = Some(node_id);
            }
            // `turn_ended` markers and unknown roles emit no node.
            _ => {}
        }
    }
    nodes
}

fn transcript_session(
    lines: &[Map<String, Value>],
    source: &CursorTranscriptSource,
    nodes: Vec<MessageNode>,
) -> Session {
    let ts_ms = source.mtime_ms.unwrap_or_else(now_ms);
    let meta = transcript_meta(lines, ts_ms);
    let mut session = session_shell(
        &source.id,
        meta.first_user_text
            .as_deref()
            .map_or_else(|| source.id.clone(), |s| s.chars().take(80).collect()),
        decode_project_dir(&source.project_slug),
    );
    session.created_at = (ts_ms / 1000.0).floor();
    session.last_activity_at = (ts_ms / 1000.0).floor();
    session.main_chain_id = nodes.last().map_or(0, |n| n.node_id);
    session
        .parent_session_id
        .clone_from(&source.parent_session_id);
    session.metadata = json!({
        "source": "cursor",
        "store": "transcript",
        "project": source.project_slug,
        "lossy": true,
        "turnErrors": meta.turn_errors,
    });
    session.nodes = nodes;
    session.prompt_history = meta.prompt_history;
    session
}

/**
 * A transcript projection into full IR: text and `tool_use` blocks map to
 * nodes, but the projection carries no tool results, usage, or per-message
 * timestamps — every node is stamped with the file mtime.
 */
pub fn from_transcript_jsonl(raw: &str, source: &CursorTranscriptSource) -> Session {
    let lines = parse_lines(raw);
    let ts_ms = source.mtime_ms.unwrap_or_else(now_ms);
    transcript_session(
        &lines,
        source,
        transcript_nodes(&lines, (ts_ms / 1000.0).floor()),
    )
}

/// The list-time shape: transcript meta without building nodes.
pub fn summarize_transcript_jsonl(raw: &str, source: &CursorTranscriptSource) -> Session {
    transcript_session(&parse_lines(raw), source, Vec::new())
}

/* ------------------------------------------------------------------ */
/* IR → agent-transcripts projection (write side)                      */
/* ------------------------------------------------------------------ */

/**
 * The project dir name Cursor derives from a working directory — the
 * inverse of `decode_project_dir`: non-alphanumerics flatten to `-` and the
 * leading separator drops (`/home/me/proj` → `home-me-proj`). Lossy the
 * same way the decode is (`my proj` and `my-proj` collide).
 */
pub fn project_slug_from_cwd(cwd: &str) -> String {
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "root".into()
    } else {
        slug.to_string()
    }
}

/// One IR node → the transcript line it projects to; `None` when the
/// format has no slot for the role (`system`, `tool` results — Cursor's own
/// writer drops them too).
fn transcript_line(node: &MessageNode) -> Option<Value> {
    match node.role {
        Role::User => {
            if node.content.is_empty() {
                return None;
            }
            Some(json!({
                "role": "user",
                "message": { "content": [{ "type": "text", "text": node.content }] },
            }))
        }
        Role::Assistant => {
            // Sealed/recorded thinking projects the way Cursor's own writer
            // renders it: a `[REDACTED]` suffix inside the text block.
            let text = if node.thinking.is_some() {
                if node.content.is_empty() {
                    "[REDACTED]".to_string()
                } else {
                    format!("{}\n\n[REDACTED]", node.content)
                }
            } else {
                node.content.clone()
            };
            let mut content: Vec<Value> = Vec::new();
            if !text.is_empty() {
                content.push(json!({ "type": "text", "text": text }));
            }
            for call in &node.tool_calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            if content.is_empty() {
                return None;
            }
            Some(json!({
                "role": "assistant",
                "message": { "content": content },
            }))
        }
        _ => None,
    }
}

/**
 * Encode a session into the transcript projection's JSONL. Real
 * transcripts carry `{role, message:{content}}` lines only, so user text,
 * assistant text and `tool_use` blocks survive while tool results, system
 * prompts, per-message timestamps, usage and the title are dropped — the
 * same loss Cursor's own projection accepts. `id` rides on `tool_use`
 * even though Cursor omits it: the reader honours it and a converted
 * session keeps its call ids.
 */
pub fn to_transcript_jsonl(session: &Session) -> String {
    let mut out = String::new();
    for node in &session.nodes {
        if let Some(line) = transcript_line(node) {
            out.push_str(&serde_json::to_string(&line).unwrap_or_default());
            out.push('\n');
        }
    }
    out
}

/* ------------------------------------------------------------------ */
/* IR → store.db (canonical write side)                               */
/* ------------------------------------------------------------------ */

/// sha256 hex of `data` — lowercase, matching `crypto.createHash("sha256")`.
fn sha256_hex(data: &[u8]) -> String {
    to_hex(&sha2::Sha256::digest(data))
}

/// md5 hex of `text` — lowercase.
fn md5_hex(text: &str) -> String {
    to_hex(&md5::Md5::digest(text.as_bytes()))
}

/// Lexical `path.resolve` — normalize `.`/`..`/empty segments without
/// touching the filesystem; relative input resolves against the process cwd.
fn resolve_path(cwd: &str) -> String {
    let combined = if cwd.starts_with('/') {
        cwd.to_string()
    } else {
        let base = std::env::current_dir()
            .map_or_else(|_| "/".to_string(), |p| p.to_string_lossy().into_owned());
        format!("{base}/{cwd}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/**
 * A blob's `blobs.id` is the lowercase hex sha256 of its bytes — verified
 * against real stores, where `e3b0c44…` (sha256 of nothing) is the empty
 * blob every store keeps and the root of a chat with no messages yet.
 */
pub fn blob_id_for(data: &[u8]) -> String {
    sha256_hex(data)
}

/**
 * The `chats/<hash>` directory is md5 of the workspace path — the agent
 * hashes `path.resolve(cwd)`, so a trailing slash or `.`/`..` segment is
 * normalised first, same as the real mapping.
 */
pub fn workspace_hash_from_cwd(cwd: &str) -> String {
    md5_hex(&resolve_path(cwd))
}

/// Inverse of `workspace_from_uri`: each path segment is URI-encoded.
pub fn workspace_uri_from_cwd(cwd: &str) -> String {
    let encoded: Vec<String> = cwd.split('/').map(encode_uri_component).collect();
    format!("file://{}", encoded.join("/"))
}

fn write_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (value & 0x7f) as u8;
        value >>= 7;
        if value > 0 {
            b |= 0x80;
        }
        out.push(b);
        if value == 0 {
            break;
        }
    }
    out
}

fn concat_bytes(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

fn len_field(field_num: u64, data: &[u8]) -> Vec<u8> {
    concat_bytes(&[
        &write_varint(field_num * 8 + 2),
        &write_varint(data.len() as u64),
        data,
    ])
}

fn int_field(field_num: u64, value: u64) -> Vec<u8> {
    concat_bytes(&[&write_varint(field_num * 8), &write_varint(value)])
}

#[derive(Clone, Debug, Default)]
pub struct CheckpointWriteInput {
    /// Ordered sha256 blob ids of the message list — field-1 entries.
    pub message_ids: Vec<String>,
    /// `file://` workspace URI — from `workspace_uri_from_cwd`.
    pub workspace: Option<String>,
    /// Client tag written at field 22; real CLI stores use `"cli"`.
    pub client: Option<String>,
}

/**
 * Synthesise a checkpoint blob. Only the load-bearing fields are written:
 * the ordered field-1 message refs, the field-9 workspace URI, the
 * field-10 flag and the field-22 client tag. A real checkpoint also
 * carries field-5 token stats and field-8 refs to ancillary record groups
 * (prompt/context/step/tool-detail blobs) — UI bookkeeping the resume
 * path never reads, deliberately omitted rather than guessed at.
 * `decode_checkpoint` round-trips everything written here.
 */
pub fn encode_checkpoint(input: &CheckpointWriteInput) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for id in &input.message_ids {
        if let Some(bytes) = from_hex(id) {
            if bytes.len() == 32 {
                parts.push(len_field(1, &bytes));
            }
        }
    }
    if let Some(workspace) = &input.workspace {
        parts.push(len_field(9, workspace.as_bytes()));
    }
    parts.push(int_field(10, 1));
    parts.push(len_field(
        22,
        input.client.as_deref().unwrap_or("cli").as_bytes(),
    ));
    parts.concat()
}

#[derive(Clone, Debug, Default)]
pub struct StoreMetaWriteInput {
    pub agent_id: String,
    pub latest_root_blob_id: String,
    pub name: Option<String>,
    pub mode: Option<String>,
    pub is_run_everything: Option<bool>,
    /// Epoch milliseconds.
    pub created_at: Option<f64>,
    pub last_used_model: Option<String>,
}

/// A JS `JSON.stringify` number — integral floats render without `.0`.
pub(crate) fn js_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() <= 9.0e15 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// The `meta['0']` row — hex of the UTF-8 JSON, matching `parse_store_meta`.
pub fn encode_store_meta(meta: &StoreMetaWriteInput) -> String {
    let mut obj = Map::new();
    obj.insert("agentId".into(), json!(meta.agent_id));
    obj.insert("latestRootBlobId".into(), json!(meta.latest_root_blob_id));
    if let Some(name) = &meta.name {
        obj.insert("name".into(), json!(name));
    }
    if let Some(mode) = &meta.mode {
        obj.insert("mode".into(), json!(mode));
    }
    if let Some(is_run_everything) = meta.is_run_everything {
        obj.insert("isRunEverything".into(), json!(is_run_everything));
    }
    if let Some(created_at) = meta.created_at {
        obj.insert("createdAt".into(), js_number(created_at));
    }
    if let Some(model) = &meta.last_used_model {
        obj.insert("lastUsedModel".into(), json!(model));
    }
    to_hex(
        serde_json::to_string(&Value::Object(obj))
            .unwrap_or_default()
            .as_bytes(),
    )
}

#[derive(Clone, Debug, Default)]
pub struct MetaJsonWriteInput {
    pub created_at_ms: f64,
    pub updated_at_ms: f64,
    pub title: Option<String>,
    pub has_conversation: bool,
    pub cwd: Option<String>,
}

/// The `meta.json` sidecar — schemaVersion 1, key order matching real files.
pub fn encode_meta_json(meta: &MetaJsonWriteInput) -> String {
    let mut obj = Map::new();
    obj.insert("schemaVersion".into(), json!(1));
    obj.insert("createdAtMs".into(), js_number(meta.created_at_ms));
    obj.insert("hasConversation".into(), json!(meta.has_conversation));
    if let Some(title) = meta.title.as_ref().filter(|t| !t.is_empty()) {
        obj.insert("title".into(), json!(title));
    }
    obj.insert("updatedAtMs".into(), js_number(meta.updated_at_ms));
    if let Some(cwd) = &meta.cwd {
        obj.insert("cwd".into(), json!(cwd));
    }
    serde_json::to_string(&Value::Object(obj)).unwrap_or_default()
}

fn node_context(node: &MessageNode) -> Option<&str> {
    node.metadata.get("context")?.as_str()
}

fn node_blob_id(node: &MessageNode) -> Option<&str> {
    node.metadata.get("blobId")?.as_str()
}

/**
 * A deterministic `providerOptions.cursor.requestId` — real stores mint a
 * random uuid per user turn, but deriving one from session+node keeps a
 * repeated `save` byte-identical: a random id would orphan a fresh message
 * and checkpoint blob on every write.
 */
fn derived_request_id(session_id: &str, node_id: i64) -> String {
    let hex = sha256_hex(format!("cursor-request:{session_id}:{node_id}").as_bytes());
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn tool_result_json(node: &MessageNode) -> Value {
    let mut obj = Map::new();
    obj.insert("type".into(), json!("tool-result"));
    if let Some(id) = &node.tool_call_id {
        obj.insert("toolCallId".into(), json!(id));
    }
    if let Some(name) = &node.tool_name {
        obj.insert("toolName".into(), json!(name));
    }
    obj.insert("result".into(), json!(node.content));
    Value::Object(obj)
}

/// `highLevelToolCallResult.output` rebuilt from the IR's status + duration.
fn tool_result_output(node: &MessageNode) -> Value {
    let info = node.tool_result.as_ref();
    let mut obj = Map::new();
    obj.insert(
        "isError".into(),
        json!(info.is_some_and(|i| i.status == ToolCallStatus::Error)),
    );
    if let Some(duration_ms) = info.and_then(|i| i.duration_ms) {
        obj.insert(
            "success".into(),
            json!({ "executionTime": js_number(duration_ms) }),
        );
    }
    Value::Object(obj)
}

/**
 * One IR node → the AI-SDK JSON it serialises to; `None` when the
 * node carries nothing a message blob can hold (empty text, an assistant
 * turn with no content, calls or thinking). Shapes mirror what the reader
 * decodes: `user` context nodes keep the raw string `content` form while
 * queries use `[{type:"text"}]` + a requestId; `assistant` packs
 * `redacted-reasoning`/`text`/`tool-call` items and records `id: "1"` like
 * every observed assistant blob; `tool` results lift the call id to the
 * top-level `id` and re-encode `toolResult` under
 * `providerOptions.cursor.highLevelToolCallResult`.
 */
fn message_json(session_id: &str, node: &MessageNode) -> Option<Value> {
    match node.role {
        Role::System => {
            if node.content.is_empty() {
                None
            } else {
                Some(json!({ "role": "system", "content": node.content }))
            }
        }
        Role::User => {
            if node.content.is_empty() {
                return None;
            }
            // `<user_info>` plumbing is a raw string in real stores, not a
            // parts list.
            if node_context(node) == Some("user_info") {
                return Some(json!({ "role": "user", "content": node.content }));
            }
            Some(json!({
                "role": "user",
                "content": [{ "type": "text", "text": node.content }],
                "providerOptions": {
                    "cursor": {
                        "requestId": node
                            .request_id
                            .clone()
                            .unwrap_or_else(|| derived_request_id(session_id, node.node_id)),
                    },
                },
            }))
        }
        Role::Assistant => {
            let mut content: Vec<Value> = Vec::new();
            if node.thinking.is_some() {
                let mut item = Map::new();
                item.insert("type".into(), json!("redacted-reasoning"));
                if let Some(signature) = &node.thinking_signature {
                    item.insert("data".into(), json!(signature));
                }
                content.push(Value::Object(item));
            }
            if !node.content.is_empty() {
                content.push(json!({ "type": "text", "text": node.content }));
            }
            for call in &node.tool_calls {
                let args = if call.arguments.is_null() {
                    json!({})
                } else {
                    call.arguments.clone()
                };
                content.push(json!({
                    "type": "tool-call",
                    "toolCallId": call.id,
                    "toolName": call.name,
                    "args": args,
                }));
            }
            if content.is_empty() {
                return None;
            }
            Some(json!({ "role": "assistant", "id": "1", "content": content }))
        }
        Role::Tool => {
            let mut obj = Map::new();
            obj.insert("role".into(), json!("tool"));
            if let Some(id) = &node.tool_call_id {
                obj.insert("id".into(), json!(id));
            }
            obj.insert("content".into(), json!([tool_result_json(node)]));
            obj.insert(
                "providerOptions".into(),
                json!({
                    "cursor": {
                        "highLevelToolCallResult": { "output": tool_result_output(node) },
                    },
                }),
            );
            Some(Value::Object(obj))
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreBlobWrite {
    /// `blobs.id` — the sha256 hex of `data`.
    pub id: String,
    pub data: Vec<u8>,
}

/**
 * The ordered message-blob plan: entry `i` is checkpoint field-1 ref `i`.
 * Consecutive `tool` nodes that a real store grouped under one blob id
 * (recorded on `metadata.blobId`) re-group into a single blob; every other
 * node is one blob each. `result` strings are the node's content verbatim.
 */
pub fn message_blobs_from_session(session: &Session) -> Vec<StoreBlobWrite> {
    struct ToolGroup<'a> {
        blob_id: String,
        results: Vec<Value>,
        node: &'a MessageNode,
    }
    let mut group: Option<ToolGroup<'_>> = None;

    let mut blobs: Vec<StoreBlobWrite> = Vec::new();
    let mut push = |json_value: &Value| {
        let data = serde_json::to_string(json_value)
            .unwrap_or_default()
            .into_bytes();
        let id = blob_id_for(&data);
        blobs.push(StoreBlobWrite { id, data });
    };

    macro_rules! flush_group {
        () => {
            if let Some(group) = group.take() {
                let mut obj = Map::new();
                obj.insert("role".into(), json!("tool"));
                if let Some(id) = &group.node.tool_call_id {
                    obj.insert("id".into(), json!(id));
                }
                obj.insert("content".into(), Value::Array(group.results));
                obj.insert(
                    "providerOptions".into(),
                    json!({
                        "cursor": {
                            "highLevelToolCallResult": {
                                "output": tool_result_output(group.node),
                            },
                        },
                    }),
                );
                push(&Value::Object(obj));
            }
        };
    }

    for node in &session.nodes {
        if node.role != Role::Tool {
            flush_group!();
            if let Some(json_value) = message_json(&session.id, node) {
                push(&json_value);
            }
            continue;
        }
        let shared = node_blob_id(node);
        let in_group = matches!(
            (shared, group.as_ref()),
            (Some(s), Some(g)) if g.blob_id == s
        );
        if in_group {
            if let Some(g) = group.as_mut() {
                g.results.push(tool_result_json(node));
            }
            continue;
        }
        flush_group!();
        if let Some(shared) = shared {
            group = Some(ToolGroup {
                blob_id: shared.to_string(),
                results: vec![tool_result_json(node)],
                node,
            });
        } else if let Some(json_value) = message_json(&session.id, node) {
            push(&json_value);
        }
    }
    flush_group!();
    blobs
}

#[derive(Clone, Debug)]
pub struct StoreWritePlan {
    /// Every row `save` inserts into `blobs` — message blobs then the root.
    pub blobs: Vec<StoreBlobWrite>,
    /// sha256 of the checkpoint — `meta['0'].latestRootBlobId`.
    pub root_blob_id: String,
    /// Hex-encoded `meta['0']` JSON row.
    pub meta_row: String,
    /// `meta.json` sidecar content.
    pub meta_json: String,
    /// `prompt_history.json` content — `None` when nothing is provable.
    pub prompt_history_json: Option<String>,
}

/**
 * Everything a `store.db` + sidecar write needs for one session. An empty
 * session gets the empty blob as its root — exactly what a fresh real
 * chat's store records (`latestRootBlobId` = sha256 of nothing).
 * `created_at_ms` lets a rewrite keep the chat's original creation time.
 */
pub fn store_write_plan(session: &Session, created_at_ms: Option<f64>) -> StoreWritePlan {
    let empty_data: Vec<u8> = Vec::new();
    let messages = message_blobs_from_session(session);
    let checkpoint = if messages.is_empty() {
        empty_data.clone()
    } else {
        encode_checkpoint(&CheckpointWriteInput {
            message_ids: messages.iter().map(|b| b.id.clone()).collect(),
            workspace: Some(workspace_uri_from_cwd(&session.working_directory)),
            client: Some("cli".into()),
        })
    };
    let root_blob_id = blob_id_for(&checkpoint);
    let metadata = session.metadata.as_object();
    let created_at_ms = created_at_ms.unwrap_or(session.created_at * 1000.0);

    let prompts: Vec<String> = if session.prompt_history.is_empty() {
        session
            .nodes
            .iter()
            .filter(|node| node.role == Role::User && node_context(node) != Some("user_info"))
            .filter_map(|node| extract_user_query(&node.content))
            .collect()
    } else {
        session
            .prompt_history
            .iter()
            .map(|entry| entry.content.clone())
            .collect()
    };

    let mode = metadata
        .and_then(|m| m.get("mode").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(|| {
            if session.agent_mode.is_empty() {
                None
            } else {
                Some(session.agent_mode.clone())
            }
        });

    let mut blobs = Vec::with_capacity(messages.len() + 2);
    blobs.push(StoreBlobWrite {
        id: blob_id_for(&empty_data),
        data: empty_data,
    });
    blobs.extend(messages);
    blobs.push(StoreBlobWrite {
        id: root_blob_id.clone(),
        data: checkpoint,
    });

    StoreWritePlan {
        blobs,
        root_blob_id: root_blob_id.clone(),
        meta_row: encode_store_meta(&StoreMetaWriteInput {
            agent_id: session.id.clone(),
            latest_root_blob_id: root_blob_id,
            name: if session.title.is_empty() {
                None
            } else {
                Some(session.title.clone())
            },
            mode,
            is_run_everything: metadata
                .and_then(|m| m.get("isRunEverything").and_then(Value::as_bool)),
            created_at: Some(created_at_ms),
            last_used_model: Some(
                if session.model.is_empty() || session.model == "unknown" {
                    "default"
                } else {
                    session.model.as_str()
                }
                .to_string(),
            ),
        }),
        meta_json: encode_meta_json(&MetaJsonWriteInput {
            created_at_ms,
            updated_at_ms: (session.last_activity_at * 1000.0).max(created_at_ms),
            title: Some(session.title.clone()),
            has_conversation: !session.nodes.is_empty(),
            cwd: Some(session.working_directory.clone()),
        }),
        prompt_history_json: if prompts.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&prompts).unwrap_or_default())
        },
    }
}
