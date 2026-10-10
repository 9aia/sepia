//! History view-model — the transcript *backlog* half, ported from the
//! TS app's `toolDisplay.ts`/`systemContext.ts`/`historyRows.ts`/`format.ts`.
//! Pure data in, pure data out: the Leptos page maps [`ChatRow`]s to
//! components and renders [`ToolDisplay`] segments verbatim.
//!
//! Two payload shapes arrive per tool row:
//!
//! - History (IR `tool` nodes): `content` is the agent's result *text* —
//!   Devin-style envelopes like `<file-view path=…>`, `Output from command
//!   in shell X:`, `The file P has been updated…`, `Found N match(es)…`.
//!   The call's args arrive as `args` too (the IR `ToolCall.arguments`,
//!   JSON-encoded); older rows lack them, so the salient bits
//!   (path/command/query) are still parsed back out of the text as well.
//! - Live (`SessionEvent`): `args` is the concatenated `raw_input` JSON
//!   (one full snapshot per update, appended back-to-back), `result` the
//!   `raw_output` JSON.
//!
//! Everything is heuristic: unrecognized names or payloads fall back to a
//! generic label + text body so nothing renders worse than before.

use std::collections::BTreeMap;

use sepia_core::{TokenUsage, ToolCallDiff};
use serde_json::Value;

/* ==== tool display ==================================================== */

/// The renderer bucket a tool name maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCategory {
    Exec,
    Edit,
    Read,
    Search,
    Fetch,
    Todo,
    Other,
}

impl ToolCategory {
    /// `data-name` suffix for the rendered block (`ToolExec`, …).
    pub fn hook(self) -> &'static str {
        match self {
            Self::Exec => "ToolExec",
            Self::Edit => "ToolEdit",
            Self::Read => "ToolRead",
            Self::Search => "ToolSearch",
            Self::Fetch => "ToolFetch",
            Self::Todo => "ToolTodo",
            Self::Other => "ToolCall",
        }
    }
}

/// One body section inside a tool `<details>`.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolSegment {
    /// A `$ command` line.
    Command(String),
    /// Monospace block — file snippets, shell output, JSON.
    Code(String),
    /// old/new lines with -/+ prefixes, colored by the renderer.
    Diff(String),
    /// Free text rendered through the markdown pipeline.
    Markdown(String),
    /// Small status line — exit code, truncation notes.
    Note { text: String, error: bool },
}

/// A tool row mapped for display: human label, salient one-line detail,
/// and the typed body segments.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDisplay {
    pub category: ToolCategory,
    /// Human label for the marker line ("Read file", "Ran command").
    pub label: String,
    /// The salient argument — path, command, or query. Mono, truncated.
    pub detail: Option<String>,
    pub segments: Vec<ToolSegment>,
}

/// VT100/xterm escape sequences — exec results carry raw terminal output.
/// Hand-rolled (no regex dep): CSI `\x1b[ … final@0x40-0x7e`, OSC
/// `\x1b] … BEL or ST(\x1b\\)`, `()`-shift and two-char escapes.
pub fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            // Consume one char (multi-byte safe).
            let rest = &text[i..];
            let len = rest.chars().next().map_or(1, char::len_utf8);
            out.push_str(&rest[..len]);
            i += len;
            continue;
        }
        i += 1;
        match bytes.get(i) {
            Some(b'[') => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += usize::from(i < bytes.len());
            }
            Some(b']') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            Some(b'(' | b')' | b'=' | b'>') => i += 2,
            _ => {}
        }
    }
    out
}

/// Exclusive end index of the JSON value starting at `start`, or `None`.
/// Only `"`/`{`/`[` openings count — the arg stream is snapshots of
/// `JSON.stringify(rawInput)` values.
fn scan_json_value(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    match bytes.get(start)? {
        b'"' => {
            let mut i = start + 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => return Some(i + 1),
                    _ => i += 1,
                }
            }
            None
        }
        b'{' | b'[' => {
            let mut depth = 0i32;
            let mut in_string = false;
            let mut i = start;
            while i < bytes.len() {
                let c = bytes[i];
                if in_string {
                    match c {
                        b'\\' => i += 1,
                        b'"' => in_string = false,
                        _ => {}
                    }
                } else {
                    match c {
                        b'"' => in_string = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1);
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            None
        }
        _ => None,
    }
}

/// Parses a run of concatenated JSON values (live args/results arrive as
/// `JSON.stringify` snapshots appended back-to-back). Returns the decoded
/// values plus whatever non-JSON text trails them — a partial snapshot
/// while still streaming lands in `rest`.
pub fn split_leading_json(text: &str) -> (Vec<Value>, &str) {
    let mut values = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let Some(&c) = bytes.get(i) else { break };
        if c != b'{' && c != b'[' && c != b'"' {
            break;
        }
        let Some(end) = scan_json_value(text, i) else {
            break;
        };
        match serde_json::from_str::<Value>(&text[i..end]) {
            Ok(v) => values.push(v),
            Err(_) => break,
        }
        i = end;
    }
    (values, &text[i..])
}

/// Last complete JSON object in an args stream, if any.
fn parse_args(raw: Option<&str>) -> Option<serde_json::Map<String, Value>> {
    let (values, _) = split_leading_json(raw?);
    values
        .into_iter()
        .rev()
        .find_map(|v| v.as_object().cloned())
}

const TEXT_FIELDS: [&str; 6] = ["content", "text", "output", "result", "stdout", "message"];

/// Pull a displayable string out of a result payload.
fn result_to_text(value: &Value) -> String {
    match value {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Object(record) => {
            for field in TEXT_FIELDS {
                if let Some(s) = record.get(field).and_then(Value::as_str) {
                    if !s.is_empty() {
                        return s.to_string();
                    }
                }
            }
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

/// Result payloads are usually plain text; live ones are one JSON value
/// (string/object) — decode it, keep any trailing non-JSON text.
fn result_text(content: &str) -> (String, Option<Value>) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }
    let first = trimmed.as_bytes()[0];
    if first == b'{' || first == b'[' || first == b'"' {
        let (values, rest) = split_leading_json(trimmed);
        if let Some(raw) = values.into_iter().last() {
            let text = result_to_text(&raw);
            let tail = rest.trim();
            return if tail.is_empty() {
                (text, Some(raw))
            } else {
                (format!("{text}\n{rest}"), Some(raw))
            };
        }
    }
    (content.to_string(), None)
}

/// `exec|bash|shell|terminal|command|getoutput|runc|cmd|powershell` →
/// exec; `edit|write|wrote|creat|patch|insert|delete|rename|move` →
/// edit; … same name-shape table the TS app used.
pub fn categorize(tool_name: &str) -> ToolCategory {
    let n: String = tool_name
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    let has = |needles: &[&str]| needles.iter().any(|p| n.contains(p));
    if has(&[
        "exec",
        "bash",
        "shell",
        "terminal",
        "command",
        "getoutput",
        "runc",
        "cmd",
        "powershell",
    ]) {
        return ToolCategory::Exec;
    }
    if has(&[
        "edit", "write", "wrote", "creat", "patch", "insert", "delete", "rename", "move",
    ]) {
        return ToolCategory::Edit;
    }
    if has(&["grep", "glob", "search", "find", "query", "list", "look"]) {
        return ToolCategory::Search;
    }
    if has(&["fetch", "browse", "web", "http", "url", "download"]) {
        return ToolCategory::Fetch;
    }
    if has(&["todo", "task", "plan", "checklist"]) {
        return ToolCategory::Todo;
    }
    if has(&["read", "view", "open", "cat", "load", "inspect"]) {
        return ToolCategory::Read;
    }
    ToolCategory::Other
}

/// "todo_write" → "Todo write"; "other"/"" → "Tool call".
fn prettify_name(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | '.' | '/') {
            spaced.push(' ');
            prev_lower = false;
        } else {
            if c.is_ascii_uppercase() && prev_lower {
                spaced.push(' ');
            }
            spaced.push(c);
            prev_lower = c.is_ascii_lowercase();
        }
    }
    let spaced = spaced.trim();
    if spaced.is_empty() || spaced.eq_ignore_ascii_case("other") {
        return "Tool call".to_string();
    }
    let mut out = spaced.to_string();
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}

fn str_field<'a>(record: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// First string of an array arg, e.g. cline's `{files: [{path}]}` /
/// `{queries: [...]}`.
fn first_string(value: Option<&Value>) -> Option<String> {
    let first = value?.as_array()?.first()?;
    if let Some(s) = first.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let record = first.as_object()?;
    str_field(record, "path")
        .or_else(|| str_field(record, "url"))
        .map(str::to_string)
}

/// `path` under common arg names (`file_path` devin/cline, `path`,
/// `files[]`).
fn arg_path(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    str_field(args, "file_path")
        .or_else(|| str_field(args, "path"))
        .or_else(|| str_field(args, "file"))
        .map(str::to_string)
        .or_else(|| first_string(args.get("files")))
}

/// Args wrapped one level deep — `{input: {command: …}}` and friends.
fn nested_command(value: Option<&Value>) -> Option<String> {
    let record = value?.as_object()?;
    str_field(record, "command")
        .or_else(|| str_field(record, "cmd"))
        .or_else(|| str_field(record, "shell"))
        .map(str::to_string)
}

fn arg_command(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    str_field(args, "command")
        .or_else(|| str_field(args, "cmd"))
        .or_else(|| str_field(args, "script"))
        // `shell` reads as a command only when none of the unambiguous
        // fields set it — it can also name the shell binary
        // (`shell: "bash"`).
        .or_else(|| str_field(args, "shell"))
        .map(str::to_string)
        .or_else(|| nested_command(args.get("input")))
        .or_else(|| nested_command(args.get("params")))
        .or_else(|| {
            let joined = args
                .get("commands")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        })
}

/// The shell id a `get_output`/`kill_shell`-style call targets.
fn arg_shell_id(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    str_field(args, "shell_id")
        .or_else(|| str_field(args, "shellId"))
        .map(str::to_string)
}

fn arg_query(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    str_field(args, "pattern")
        .or_else(|| str_field(args, "query"))
        .or_else(|| str_field(args, "regex"))
        .map(str::to_string)
        .or_else(|| first_string(args.get("queries")))
}

fn arg_url(args: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    let args = args?;
    str_field(args, "url")
        .or_else(|| str_field(args, "query"))
        .or_else(|| str_field(args, "request"))
        .map(str::to_string)
        .or_else(|| first_string(args.get("requests")))
}

/// `:start–end` range suffix from `start_line`/`offset` + `end_line`/`limit`.
fn arg_range(args: &serde_json::Map<String, Value>) -> String {
    let start = args
        .get("start_line")
        .or_else(|| args.get("offset"))
        .and_then(Value::as_f64);
    let end = args.get("end_line").and_then(Value::as_f64).or_else(|| {
        start.and_then(|s| {
            args.get("limit")
                .and_then(Value::as_f64)
                .map(|l| s + l - 1.0)
        })
    });
    match (start, end) {
        (Some(s), Some(e)) => format!(":{s:.0}–{e:.0}"),
        _ => String::new(),
    }
}

/// Cap display size — history results can be thousands of lines.
fn cap_lines(text: &str, max: usize) -> String {
    let total = text.lines().count();
    if total <= max {
        return text.to_string();
    }
    let head: String = text.lines().take(max).collect::<Vec<_>>().join("\n");
    format!("{head}\n… ({} more lines)", total - max)
}

/// Drop `NN|` line-number prefixes — content keeps its own indentation.
fn strip_line_numbers(text: &str) -> String {
    text.lines()
        .map(|line| {
            let t = line.trim_start();
            let digits = t.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=6).contains(&digits) && t.as_bytes().get(digits) == Some(&b'|') {
                &t[digits + 1..]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/* ---- result-text envelopes seen in stored sessions (devin + cline) --- */

/// `<file-view path="…" [start_line="N"] [end_line="M"]>` envelope →
/// `(path, range, body)`.
fn parse_file_view(text: &str) -> Option<(Option<String>, String, &str)> {
    let rest = text.trim().strip_prefix("<file-view")?;
    let gt = rest.find('>')?;
    let attrs = &rest[..gt];
    let attr = |key: &str| -> Option<String> {
        let needle = format!("{key}=\"");
        let pos = attrs.find(&needle)? + needle.len();
        let end = attrs[pos..].find('"')? + pos;
        Some(attrs[pos..end].to_string())
    };
    let path = attr("path");
    let range = match (attr("start_line"), attr("end_line")) {
        (Some(s), Some(e)) => format!(":{s}–{e}"),
        _ => String::new(),
    };
    Some((path, range, rest[gt + 1..].trim_start()))
}

/// `The file P has been updated.…edited file:\n<body>` → `(path, body)`.
fn parse_edit_updated(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix("The file ")?;
    let path_end = rest.find(' ')?;
    let path = &rest[..path_end];
    if !rest[path_end..].starts_with(" has been updated.") {
        return None;
    }
    let marker = rest.find("edited file:\n")?;
    Some((path.to_string(), &rest[marker + "edited file:\n".len()..]))
}

/// `File created successfully at: P` → `path`.
fn parse_created(text: &str) -> Option<String> {
    let rest = text
        .trim_start()
        .strip_prefix("File created successfully at:")?;
    Some(rest.split_whitespace().next()?.to_string())
}

/// `Output from command in shell X:` prefix — returns the body after it.
fn strip_exec_header(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("Output from command in shell ") else {
        return text;
    };
    match rest.find(':') {
        Some(i) if !rest[..i].contains('\n') => {
            rest[i + 1..].strip_prefix('\n').unwrap_or(&rest[i + 1..])
        }
        _ => text,
    }
}

/// Trailing `\n{0,3}Exit code: N` → `(code, body-without-it)`.
fn split_exit_code(text: &str) -> (Option<i64>, &str) {
    let tail = text.trim_end();
    let Some(pos) = tail.rfind("Exit code:") else {
        return (None, text);
    };
    let num = tail[pos + "Exit code:".len()..].trim();
    // `-?digits` and nothing else may follow.
    let digits = num.strip_prefix('-').unwrap_or(num);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (None, text);
    }
    // At most three newlines may precede the marker.
    let before = &tail[..pos];
    if before.chars().rev().take_while(|c| *c == '\n').count() > 3 {
        return (None, text);
    }
    let code = num.parse::<i64>().ok();
    (code, tail[..pos].trim_end_matches('\n'))
}

/// Leading `… (N line[s] truncated)` note → `(count, body)`.
fn split_trunc_lines(text: &str) -> (Option<String>, &str) {
    let t = text.trim_start();
    let Some(rest) = t.strip_prefix("… (") else {
        return (None, text);
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return (None, text);
    }
    let rest = &rest[digits.len()..];
    for suffix in [" lines truncated)", " line truncated)"] {
        if let Some(after) = rest.strip_prefix(suffix) {
            return (Some(digits), after.strip_prefix('\n').unwrap_or(after));
        }
    }
    (None, text)
}

/// `<truncation_notice>Full output written to: PATH</truncation_notice>`
/// → `(path, body)`.
fn strip_trunc_notice(text: &str) -> (Option<String>, String) {
    const OPEN: &str = "<truncation_notice>";
    const CLOSE: &str = "</truncation_notice>";
    let Some(start) = text.find(OPEN) else {
        return (None, text.to_string());
    };
    let body_start = start + OPEN.len();
    let Some(end) = text[body_start..].find(CLOSE).map(|e| e + body_start) else {
        return (None, text.to_string());
    };
    let inner = text[body_start..end].trim();
    let path = inner
        .strip_prefix("Full output written to:")
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let kept = format!("{}{}", &text[..start], &text[end + CLOSE.len()..]);
    (path.map(str::to_string), kept)
}

/// Trailing `` `cmd` was parsed out (N of M [total ]lines shown). `` —
/// devin's shell trims giant output with this note; returns the note
/// text and the body without it.
fn split_parsed_out(text: &str) -> (Option<String>, &str) {
    let tail = text.trim_end();
    if !tail.ends_with("lines shown).") {
        return (None, text);
    }
    // The note opens at the backtick before `… was parsed out`.
    let Some(pos) = tail.rfind("` was parsed out (") else {
        return (None, text);
    };
    let Some(tick) = tail[..pos].rfind('`') else {
        return (None, text);
    };
    let note = &tail[tick..];
    // Validate the counts: `(N of M [total ]lines shown).`
    let Some(open) = note.find(" was parsed out (") else {
        return (None, text);
    };
    let counts = &note[open + " was parsed out (".len()..];
    let Some(close) = counts.find(')') else {
        return (None, text);
    };
    let counts = counts[..close].replace("total ", "");
    let mut parts = counts.split(" of ");
    let valid = matches!(
        (parts.next(), parts.next()),
        (Some(a), Some(b))
            if !a.is_empty()
                && a.bytes().all(|x| x.is_ascii_digit())
                && !b.is_empty()
                && b.bytes().all(|x| x.is_ascii_digit())
    );
    if !valid {
        return (None, text);
    }
    let cleaned = note.trim().replace('`', "");
    (Some(cleaned), &text[..tick])
}

/// `Found N match(es) for pattern 'Q' in SCOPE:\n<body>`.
fn parse_search_result(text: &str) -> Option<(String, String, String, &str)> {
    let rest = text.strip_prefix("Found ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = rest[digits.len()..].strip_prefix(" match(es) for pattern '")?;
    let qend = rest.find('\'')?;
    let pattern = &rest[..qend];
    let rest = rest[qend + 1..].strip_prefix(" in ")?;
    let split = rest.find(":\n")?;
    let scope = &rest[..split];
    Some((
        digits,
        pattern.to_string(),
        scope.to_string(),
        &rest[split + 2..],
    ))
}

/// `# Web Search Results for "Q"` → `query`.
fn parse_fetch_query(text: &str) -> Option<String> {
    let rest = text.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("Web Search Results for \"")?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

struct ExecParsed {
    output: String,
    notes: Vec<ToolSegment>,
}

fn parse_exec_result(text: &str) -> ExecParsed {
    let mut notes = Vec::new();
    let stripped = strip_ansi(text);
    let body = strip_exec_header(&stripped).to_string();

    let (trunc_path, body) = strip_trunc_notice(&body);
    if let Some(path) = trunc_path {
        notes.push(ToolSegment::Note {
            text: format!("Full output: {path}"),
            error: false,
        });
    }
    let (parsed_out, body) = split_parsed_out(&body);
    if let Some(note) = parsed_out {
        notes.push(ToolSegment::Note {
            text: note,
            error: false,
        });
    }

    let (exit_code, body) = split_exit_code(body);
    let body = body.to_string();

    let (truncated, body) = split_trunc_lines(&body);
    if let Some(n) = truncated {
        notes.insert(
            0,
            ToolSegment::Note {
                text: format!("… {n} earlier lines truncated"),
                error: false,
            },
        );
    }

    if let Some(code) = exit_code {
        notes.push(ToolSegment::Note {
            text: format!("exit {code}"),
            error: code != 0,
        });
    }

    ExecParsed {
        output: body.trim().to_string(),
        notes,
    }
}

fn build_exec(
    tool_name: &str,
    args: Option<&serde_json::Map<String, Value>>,
    result: &(String, Option<Value>),
) -> ToolDisplay {
    let command = arg_command(args);
    let raw_exit = result
        .1
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|record| {
            ["exitCode", "exit_code", "exit", "code"]
                .iter()
                .find_map(|k| record.get(*k).and_then(Value::as_i64))
        });

    let parsed = parse_exec_result(&result.0);
    let mut segments = Vec::new();
    if let Some(cmd) = &command {
        segments.push(ToolSegment::Command(cmd.clone()));
    }
    if !parsed.output.is_empty() {
        segments.push(ToolSegment::Code(cap_lines(&parsed.output, 200)));
    }
    let has_exit_note = parsed
        .notes
        .iter()
        .any(|n| matches!(n, ToolSegment::Note { text, .. } if text.starts_with("exit ")));
    segments.extend(parsed.notes);
    if let Some(code) = raw_exit {
        if !has_exit_note {
            segments.push(ToolSegment::Note {
                text: format!("exit {code}"),
                error: code != 0,
            });
        }
    }

    let label = if tool_name
        .to_lowercase()
        .replace('_', "")
        .contains("getoutput")
    {
        "Read shell output"
    } else if tool_name
        .to_lowercase()
        .replace('_', "")
        .contains("killshell")
    {
        "Killed shell"
    } else {
        "Ran command"
    };
    // A call that only carries a shell id (get_output, kill_shell)
    // still gets a detail — it just isn't a `$ command` line.
    let detail = command
        .and_then(|c| c.lines().next().map(str::to_string))
        .or_else(|| arg_shell_id(args).map(|id| format!("shell {id}")));
    ToolDisplay {
        category: ToolCategory::Exec,
        label: label.to_string(),
        detail,
        segments,
    }
}

fn build_read(
    args: Option<&serde_json::Map<String, Value>>,
    result: &(String, Option<Value>),
) -> ToolDisplay {
    let arg_detail = arg_path(args).map(|p| {
        let range = args.map_or_else(String::new, arg_range);
        format!("{p}{range}")
    });
    let Some((path, range, body)) = parse_file_view(&result.0) else {
        // Live row before the result lands, or an unrecognized read.
        let mut segments = Vec::new();
        if !result.0.trim().is_empty() {
            segments.push(ToolSegment::Code(cap_lines(result.0.trim(), 200)));
        }
        return ToolDisplay {
            category: ToolCategory::Read,
            label: "Read file".to_string(),
            detail: arg_detail,
            segments,
        };
    };
    let detail = path.map(|p| format!("{p}{range}")).or(arg_detail);
    let code = strip_line_numbers(body);
    let code = code.trim_end_matches('\n');
    ToolDisplay {
        category: ToolCategory::Read,
        label: "Read file".to_string(),
        detail,
        segments: if code.is_empty() {
            Vec::new()
        } else {
            vec![ToolSegment::Code(cap_lines(code, 300))]
        },
    }
}

fn build_diff_text(old_text: &str, new_text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !old_text.is_empty() {
        lines.extend(cap_lines(old_text, 60).lines().map(|l| format!("- {l}")));
    }
    if !new_text.is_empty() {
        lines.extend(cap_lines(new_text, 60).lines().map(|l| format!("+ {l}")));
    }
    lines.join("\n")
}

fn build_edit(
    tool_name: &str,
    args: Option<&serde_json::Map<String, Value>>,
    result: &(String, Option<Value>),
) -> ToolDisplay {
    let text = result.0.trim();
    let updated = parse_edit_updated(text);
    let created = parse_created(text);
    let path = updated
        .as_ref()
        .map(|(p, _)| p.clone())
        .or_else(|| created.clone())
        .or_else(|| arg_path(args));
    let normalized: String = tool_name
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_lowercase)
        .collect();
    let is_write = created.is_some()
        || normalized.contains("write")
        || normalized.contains("wrote")
        || normalized.contains("creat");
    let label = if is_write {
        "Wrote file"
    } else {
        "Edited file"
    };

    let mut segments = Vec::new();
    if let Some((_, body)) = &updated {
        let snippet = strip_line_numbers(body);
        let snippet = snippet.trim_end_matches('\n');
        if !snippet.is_empty() {
            segments.push(ToolSegment::Code(cap_lines(snippet, 200)));
        }
    } else if created.is_some() {
        segments.push(ToolSegment::Note {
            text: "File created".to_string(),
            error: false,
        });
    } else if let Some(args) = args {
        let old_text = str_field(args, "old_string").or_else(|| str_field(args, "old_text"));
        let new_text = str_field(args, "new_string")
            .or_else(|| str_field(args, "new_text"))
            .or_else(|| str_field(args, "content"));
        if old_text.is_some() || new_text.is_some() {
            segments.push(ToolSegment::Diff(build_diff_text(
                old_text.unwrap_or(""),
                new_text.unwrap_or(""),
            )));
        }
    }
    if segments.is_empty() && !text.is_empty() {
        segments.push(ToolSegment::Markdown(cap_lines(text, 120)));
    }
    ToolDisplay {
        category: ToolCategory::Edit,
        label: label.to_string(),
        detail: path,
        segments,
    }
}

fn build_search(
    args: Option<&serde_json::Map<String, Value>>,
    result: &(String, Option<Value>),
) -> ToolDisplay {
    let text = result.0.trim();
    let query = arg_query(args);
    if let Some((count, pattern, scope, body)) = parse_search_result(text) {
        return ToolDisplay {
            category: ToolCategory::Search,
            label: "Searched codebase".to_string(),
            detail: Some(pattern),
            segments: vec![
                ToolSegment::Note {
                    text: format!("{count} match(es) in {scope}"),
                    error: false,
                },
                ToolSegment::Code(cap_lines(body.trim(), 200)),
            ],
        };
    }
    ToolDisplay {
        category: ToolCategory::Search,
        label: "Searched codebase".to_string(),
        detail: query,
        segments: if text.is_empty() {
            Vec::new()
        } else {
            vec![ToolSegment::Code(cap_lines(text, 200))]
        },
    }
}

fn build_fetch(
    args: Option<&serde_json::Map<String, Value>>,
    result: &(String, Option<Value>),
) -> ToolDisplay {
    let text = result.0.trim();
    let query = arg_url(args).or_else(|| parse_fetch_query(text));
    ToolDisplay {
        category: ToolCategory::Fetch,
        label: "Fetched web content".to_string(),
        detail: query,
        segments: if text.is_empty() {
            Vec::new()
        } else {
            vec![ToolSegment::Markdown(cap_lines(text, 150))]
        },
    }
}

fn build_generic(tool_name: &str, result: &(String, Option<Value>)) -> ToolDisplay {
    let text = result.0.trim();
    // Structured result we didn't pull text out of — show it
    // pretty-printed.
    if let Some(raw) = &result.1 {
        if !raw.is_string() {
            return ToolDisplay {
                category: ToolCategory::Other,
                label: prettify_name(tool_name),
                detail: None,
                segments: vec![ToolSegment::Code(cap_lines(
                    &serde_json::to_string_pretty(raw).unwrap_or_default(),
                    120,
                ))],
            };
        }
    }
    ToolDisplay {
        category: ToolCategory::Other,
        label: prettify_name(tool_name),
        detail: None,
        segments: if text.is_empty() {
            Vec::new()
        } else {
            vec![ToolSegment::Markdown(cap_lines(text, 150))]
        },
    }
}

/// Maps a tool row to a label, a one-line detail, and typed body
/// segments. `args_json` is the live arg stream (or a history row's
/// JSON-encoded `ToolCall.arguments`); `content` is the stored/live
/// result text. `exit_code` (IR v2) is authoritative — it replaces any
/// `exit N` note parsed out of the content.
pub fn tool_summary(
    tool_name: &str,
    args_json: Option<&str>,
    content: &str,
    exit_code: Option<i64>,
) -> ToolDisplay {
    let args = parse_args(args_json);
    let result = result_text(content);
    let mut display = match categorize(tool_name) {
        ToolCategory::Exec => build_exec(tool_name, args.as_ref(), &result),
        ToolCategory::Read => build_read(args.as_ref(), &result),
        ToolCategory::Edit => build_edit(tool_name, args.as_ref(), &result),
        ToolCategory::Search => build_search(args.as_ref(), &result),
        ToolCategory::Fetch => build_fetch(args.as_ref(), &result),
        ToolCategory::Todo => ToolDisplay {
            label: "Updated todos".to_string(),
            ..build_generic(tool_name, &result)
        },
        ToolCategory::Other => build_generic(tool_name, &result),
    };
    let Some(code) = exit_code else {
        return display;
    };
    // The authoritative code replaces any `exit N` note parsed from text.
    display.segments.retain(|s| {
        !matches!(s, ToolSegment::Note { text, .. } if text.starts_with("exit ") && text[5..].trim().parse::<i64>().is_ok())
    });
    display.segments.push(ToolSegment::Note {
        text: format!("exit {code}"),
        error: code != 0,
    });
    display
}

/* ==== recorded file diffs ============================================= */

const DIFF_CONTEXT: usize = 3;
const DIFF_MAX_MIDDLE: usize = 120;

/// One file's recorded before/after rendered for the tool row: common
/// leading/trailing lines elide to `⋮` markers (with a few lines of
/// context) so a whole-file `oldText`/`newText` pair still reads as the
/// hunk it was. `added`/`removed` count the lines the elision kept, i.e.
/// the region that actually changed — honest stats, not a Myers diff.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDiffView {
    pub path: String,
    /// Display text — `- `/`+ `/`⋮ `/`  `-prefixed lines.
    pub text: String,
    pub added: usize,
    pub removed: usize,
}

pub fn file_diff_view(diff: &ToolCallDiff) -> FileDiffView {
    let old_lines: Vec<&str> = diff
        .old_text
        .as_deref()
        .map_or(Vec::new(), |t| t.split('\n').collect());
    let new_lines: Vec<&str> = diff
        .new_text
        .as_deref()
        .map_or(Vec::new(), |t| t.split('\n').collect());

    let mut pre = 0usize;
    let max_pre = old_lines.len().min(new_lines.len());
    while pre < max_pre && old_lines[pre] == new_lines[pre] {
        pre += 1;
    }
    let mut suf = 0usize;
    let max_suf = max_pre - pre;
    while suf < max_suf
        && old_lines[old_lines.len() - 1 - suf] == new_lines[new_lines.len() - 1 - suf]
    {
        suf += 1;
    }

    let mut old_mid = &old_lines[pre..old_lines.len() - suf];
    let mut new_mid = &new_lines[pre..new_lines.len() - suf];
    let removed = old_mid.len();
    let added = new_mid.len();
    let mut clipped = 0usize;
    if old_mid.len() > DIFF_MAX_MIDDLE {
        clipped += old_mid.len() - DIFF_MAX_MIDDLE;
        old_mid = &old_mid[..DIFF_MAX_MIDDLE];
    }
    if new_mid.len() > DIFF_MAX_MIDDLE {
        clipped += new_mid.len() - DIFF_MAX_MIDDLE;
        new_mid = &new_mid[..DIFF_MAX_MIDDLE];
    }

    let mut out: Vec<String> = Vec::new();
    if pre > 0 {
        out.push(format!(
            "⋮ {pre} unchanged line{}",
            if pre == 1 { "" } else { "s" }
        ));
        let from = pre.saturating_sub(DIFF_CONTEXT);
        for line in &old_lines[from..pre] {
            out.push(format!("  {line}"));
        }
    }
    out.extend(old_mid.iter().map(|l| format!("- {l}")));
    out.extend(new_mid.iter().map(|l| format!("+ {l}")));
    if clipped > 0 {
        out.push(format!("⋮ {clipped} more changed lines"));
    }
    if suf > 0 {
        let tail_start = old_lines.len() - suf;
        let tail_end = (tail_start + DIFF_CONTEXT).min(old_lines.len());
        for line in &old_lines[tail_start..tail_end] {
            out.push(format!("  {line}"));
        }
        out.push(format!(
            "⋮ {suf} unchanged line{}",
            if suf == 1 { "" } else { "s" }
        ));
    }
    FileDiffView {
        path: diff.path.clone(),
        text: out.join("\n"),
        added,
        removed,
    }
}

/// Supplemental `content` entries of a live tool call (the ACP
/// `ToolCallContent` kinds beyond `diff`) as body segments: a terminal
/// ref renders as a muted "Terminal \<id\>" note — plus a `code` block
/// when the agent inlined the terminal's text — an embedded text block
/// as a `code` block, and an image as a markdown image.
pub fn tool_content_segments(contents: &[Value]) -> Vec<ToolSegment> {
    let mut segments = Vec::new();
    for entry in contents {
        let ty = entry.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "terminal" => {
                let id = entry
                    .get("terminalId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                segments.push(ToolSegment::Note {
                    text: format!("Terminal {id}"),
                    error: false,
                });
                if let Some(output) = entry.get("output").and_then(Value::as_str) {
                    if !output.trim().is_empty() {
                        segments.push(ToolSegment::Code(cap_lines(output, 200)));
                    }
                }
            }
            "text" => {
                if let Some(text) = entry.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        segments.push(ToolSegment::Code(cap_lines(text, 200)));
                    }
                }
            }
            // `{type:"content", content:{type:"text",text}}` — the ACP
            // wrapper; unwrap one level and re-handle.
            "content" => {
                if let Some(inner) = entry.get("content") {
                    segments.extend(tool_content_segments(std::slice::from_ref(inner)));
                }
            }
            "image" => {
                let src = entry
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        entry.get("data").and_then(Value::as_str).map(|data| {
                            let mime = entry
                                .get("mimeType")
                                .and_then(Value::as_str)
                                .unwrap_or("image/png");
                            format!("data:{mime};base64,{data}")
                        })
                    });
                if let Some(src) = src {
                    segments.push(ToolSegment::Markdown(format!("![tool output]({src})")));
                }
            }
            _ => {}
        }
    }
    segments
}

/* ==== system context =================================================== */

/// A rule file declared in a `<rules>` block — `content` is the `<rule>`
/// body.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextRule {
    pub name: String,
    pub path: String,
    /// The rule body verbatim — `None` when the `<rule>` tag is empty.
    pub content: Option<String>,
}

/// A skill advertised via `<available_skills>` `- **name**: description
/// (source: …)` lines.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextSkill {
    pub name: String,
    pub description: Option<String>,
    /// The `(source: …)` suffix verbatim — usually a SKILL.md path or a
    /// builtin id.
    pub source: Option<String>,
}

/// A background-subagent report lifted out of
/// `<subagent_completion_notification>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextReport {
    /// Derived label — the report's first line (`#` heading marks
    /// stripped). Only present when the report runs past one line; the
    /// title line is lifted out of `body`.
    pub title: Option<String>,
    /// The report text — the bracketed `[…]` notification header is
    /// removed.
    pub body: String,
    /// `agent_id=…` from the `[Background subagent …]` header, when
    /// present.
    pub agent_id: Option<String>,
    /// The system node's `created_at` (epoch ms), when the store
    /// recorded one.
    pub at: Option<f64>,
}

/// One `prompt_text` slice under a heading at the prompt's minimum
/// heading depth.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptSection {
    pub title: String,
    pub body: String,
}

/// Context parsed out of agent-stored system nodes (devin's
/// `system_info` + rules).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemContext {
    pub workspaces: Vec<String>,
    pub platform: Option<String>,
    pub os_version: Option<String>,
    pub date: Option<String>,
    pub rules: Vec<ContextRule>,
    /// Advertised skills — empty when no `<available_skills>` block
    /// parsed.
    pub skills: Vec<ContextSkill>,
    /// Background-subagent reports delivered as system notifications.
    pub reports: Vec<ContextReport>,
    /// The rest of the system text — the agent's own prompt.
    pub prompt_text: String,
    /// `prompt_text` pre-split on its shallowest markdown headings
    /// (devin's `# Modes` / `# Style`) — additive; `prompt_text` stays
    /// whole.
    pub prompt_sections: Vec<PromptSection>,
}

impl SystemContext {
    /// `build_rows` skips the context card when nothing parsed.
    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
            && self.rules.is_empty()
            && self.reports.is_empty()
            && self.prompt_text.is_empty()
            && self.platform.is_none()
    }
}

/// Extract `<tag …>body</tag>` occurrences — `drain` returns the text
/// with the blocks removed plus each block's `(open-tag-attrs, body)`.
/// The open tag matches `<tag` followed by `>` or whitespace.
fn drain_tag(text: &str, tag: &str) -> (String, Vec<(String, String)>) {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut bodies = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = start + open.len();
        let next = rest.as_bytes().get(after).copied();
        // `<tagged>` must not match `<tag`.
        if !matches!(next, Some(b'>' | b' ' | b'\t' | b'\n') | None) {
            // Not this tag — copy through the `<` and keep scanning.
            out.push_str(&rest[..=start]);
            rest = &rest[start + 1..];
            continue;
        }
        let Some(gt) = rest[after..].find('>').map(|g| g + after) else {
            break;
        };
        let attrs = rest[after..gt].to_string();
        let Some(cend) = rest[gt + 1..].find(&close).map(|c| c + gt + 1) else {
            // Unclosed — treat the remainder as literal.
            break;
        };
        out.push_str(&rest[..start]);
        bodies.push((attrs, rest[gt + 1..cend].to_string()));
        rest = &rest[cend + close.len()..];
    }
    out.push_str(rest);
    (out, bodies)
}

/// `name="v"` / `name='v'` / `name=v` attr lookup inside a tag's attr
/// run.
fn tag_attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let mut pos = 0;
    while let Some(found) = attrs[pos..].find(&needle) {
        let start = pos + found;
        // Must be at a word boundary (`xname=` shouldn't match `name=`).
        let bounded = start == 0
            || attrs[..start]
                .chars()
                .last()
                .is_some_and(|c| c.is_whitespace() || c == '<');
        let vstart = start + needle.len();
        if bounded {
            return match attrs.as_bytes().get(vstart) {
                Some(b'"') => attrs[vstart + 1..]
                    .find('"')
                    .map(|e| &attrs[vstart + 1..vstart + 1 + e]),
                Some(b'\'') => attrs[vstart + 1..]
                    .find('\'')
                    .map(|e| &attrs[vstart + 1..vstart + 1 + e]),
                _ => attrs[vstart..]
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .map(|e| &attrs[vstart..vstart + e]),
            };
        }
        pos = vstart;
    }
    None
}

/// `Current workspace directories:` / `Platform:` / `OS Version:` /
/// `Today's date:` lines of a `<system_info>` body.
fn parse_system_info(
    info: &str,
    workspaces: &mut Vec<String>,
    fields: &mut BTreeMap<String, String>,
) {
    let mut in_workspaces = false;
    for line in info.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Current workspace directories") {
            in_workspaces = true;
            continue;
        }
        if in_workspaces && !trimmed.is_empty() && line.starts_with("  ") {
            let cwd = trimmed.trim_end_matches("(cwd)").trim_end().to_string();
            if !workspaces.contains(&cwd) {
                workspaces.push(cwd);
            }
            continue;
        }
        in_workspaces = false;
        for key in ["Platform", "OS Version", "Today's date"] {
            if let Some(rest) = trimmed.strip_prefix(key) {
                if let Some(value) = rest.strip_prefix(':').map(str::trim) {
                    if !value.is_empty() {
                        fields.insert(key.to_string(), value.to_string());
                    }
                }
            }
        }
    }
}

/// One notification body → a report. The `[Background subagent with
/// agent_id=… completed]` header line comes off; the report's first
/// line becomes `title` when a body remains after it.
fn parse_report(raw: &str, created_at: f64) -> Option<ContextReport> {
    let mut body = raw.to_string();
    let mut agent_id = None;
    // `[…]` header line at the top.
    if body.starts_with('[') {
        if let Some(end) = body.find(']') {
            if !body[..end].contains('\n') {
                let header = body[..=end].to_string();
                if let Some(pos) = header.find("agent_id=") {
                    let id: String = header[pos + "agent_id=".len()..]
                        .chars()
                        .take_while(|c| !c.is_whitespace() && *c != ']')
                        .collect();
                    if !id.is_empty() {
                        agent_id = Some(id);
                    }
                }
                body = body[end + 1..].trim_start().to_string();
            }
        }
    }
    let body = body.trim().to_string();
    if body.is_empty() {
        return None;
    }
    let mut title = None;
    if let Some(newline) = body.find('\n') {
        let first = body[..newline].trim().trim_start_matches('#').trim();
        // `body.trim()` guarantees text after the first newline.
        let rest = body[newline + 1..].trim().to_string();
        if !first.is_empty() {
            title = Some(first.to_string());
        }
        return Some(ContextReport {
            title,
            body: rest,
            agent_id,
            at: (created_at.is_finite() && created_at > 0.0).then_some(created_at),
        });
    }
    Some(ContextReport {
        title,
        body,
        agent_id,
        at: (created_at.is_finite() && created_at > 0.0).then_some(created_at),
    })
}

/// `prompt_text` split into headed sections at the prompt's shallowest
/// heading depth — `# Modes`, `# Style`, … — with deeper headings left
/// inside each section body. Headings inside fenced code don't split.
fn split_prompt_sections(prompt: &str) -> Vec<PromptSection> {
    let lines: Vec<&str> = prompt.lines().collect();
    let mut heads: Vec<(usize, usize, String)> = Vec::new();
    let mut fence: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start_matches([' ', '\t']);
        let leading = line.len() - t.len();
        if leading <= 3 && (t.starts_with("```") || t.starts_with("~~~")) {
            let mark: String = t.chars().take(3).collect();
            match &fence {
                None => fence = Some(mark),
                Some(f) if *f == mark => fence = None,
                _ => {}
            }
            continue;
        }
        if fence.is_some() || leading > 3 {
            continue;
        }
        let hashes = t.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
            heads.push((i, hashes, t[hashes + 1..].trim_end().to_string()));
        }
    }
    if heads.is_empty() {
        return Vec::new();
    }
    let min_depth = heads.iter().map(|h| h.1).min().unwrap_or(0);
    let tops: Vec<&(usize, usize, String)> = heads.iter().filter(|h| h.1 == min_depth).collect();
    let mut sections = Vec::new();
    for (i, top) in tops.iter().enumerate() {
        let end = tops.get(i + 1).map_or(lines.len(), |h| h.0);
        let body = lines[top.0 + 1..end].join("\n").trim().to_string();
        if !body.is_empty() {
            sections.push(PromptSection {
                title: top.2.clone(),
                body,
            });
        }
    }
    sections
}

/// `<rule name="…" path="…">body</rule>` entries inside a `<rules>` body.
fn parse_rules_body(body: &str, rules: &mut BTreeMap<String, ContextRule>) {
    let (_, entries) = drain_tag(body, "rule");
    for (attrs, content) in entries {
        let (Some(name), Some(path)) = (
            tag_attr(&attrs, "name").map(str::to_string),
            tag_attr(&attrs, "path").map(str::to_string),
        ) else {
            continue;
        };
        let content = content.trim().to_string();
        let key = format!("{name}:{path}");
        match rules.get_mut(&key) {
            // A later block may carry the body the first occurrence
            // lacked.
            Some(existing) => {
                if existing.content.is_none() && !content.is_empty() {
                    existing.content = Some(content);
                }
            }
            None => {
                rules.insert(
                    key,
                    ContextRule {
                        name,
                        path,
                        content: (!content.is_empty()).then_some(content),
                    },
                );
            }
        }
    }
}

/// `- **name**: description (source: …)` lines of `<available_skills>`.
fn parse_skills_body(body: &str, skills: &mut BTreeMap<String, ContextSkill>) {
    for line in body.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('-').map(str::trim_start) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("**") else {
            continue;
        };
        let Some(end) = rest.find("**:") else {
            continue;
        };
        let name = rest[..end].trim().to_string();
        if name.is_empty() || skills.contains_key(&name) {
            continue;
        }
        let mut description = rest[end + 3..].trim().to_string();
        let mut source = None;
        // Trailing `(source: …)`.
        if description.ends_with(')') {
            if let Some(pos) = description.rfind("(source:") {
                source = Some(
                    description[pos + "(source:".len()..]
                        .trim_end_matches(')')
                        .trim()
                        .to_string(),
                );
                description = description[..pos].trim().to_string();
            }
        }
        skills.insert(
            name.clone(),
            ContextSkill {
                name,
                description: (!description.is_empty()).then_some(description),
                source: source.filter(|s| !s.is_empty()),
            },
        );
    }
}

/// Folds system-role message bodies into one display context (dupes
/// collapse). `messages` is `(content, created_at)` pairs in transcript
/// order.
pub fn parse_system_context(messages: &[(String, f64)]) -> SystemContext {
    let mut workspaces: Vec<String> = Vec::new();
    let mut fields = BTreeMap::new();
    let mut rules = BTreeMap::new();
    let mut skills = BTreeMap::new();
    let mut reports = Vec::new();
    let mut seen_reports = std::collections::BTreeSet::new();
    let mut rest_parts: Vec<String> = Vec::new();
    for (content, created_at) in messages {
        let mut text = content.clone();
        let (t, blocks) = drain_tag(&text, "subagent_completion_notification");
        text = t;
        for (_, body) in blocks {
            let raw = body.trim();
            if raw.is_empty() || seen_reports.contains(raw) {
                continue;
            }
            if let Some(report) = parse_report(raw, *created_at) {
                seen_reports.insert(raw.to_string());
                reports.push(report);
            }
        }
        let (t, blocks) = drain_tag(&text, "system_info");
        text = t;
        for (_, info) in blocks {
            parse_system_info(&info, &mut workspaces, &mut fields);
        }
        let (t, blocks) = drain_tag(&text, "rules");
        text = t;
        for (_, body) in blocks {
            parse_rules_body(&body, &mut rules);
        }
        let (t, blocks) = drain_tag(&text, "available_skills");
        text = t;
        for (_, body) in blocks {
            parse_skills_body(&body, &mut skills);
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            rest_parts.push(trimmed.to_string());
        }
    }
    let prompt_text = rest_parts.join("\n\n");
    let prompt_sections = split_prompt_sections(&prompt_text);
    SystemContext {
        workspaces,
        platform: fields.get("Platform").cloned(),
        os_version: fields.get("OS Version").cloned(),
        date: fields.get("Today's date").cloned(),
        rules: rules.into_values().collect(),
        skills: skills.into_values().collect(),
        reports,
        prompt_text,
        prompt_sections,
    }
}

/* ==== chat rows ======================================================== */

/// One run span: which agent on which Sepia node continued a session —
/// `SummaryWire.spans`.
#[derive(Clone, Debug, PartialEq)]
pub struct RunSpan {
    /// Epoch milliseconds when the span was recorded (attach time).
    pub at: f64,
    pub agent: String,
    pub node: String,
}

/// The transcript as a flat render list — history rows by index, a
/// rolled-up system-context card, and thin provenance markers.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatRow {
    /// All `system` messages folded into one card at the top.
    Context(SystemContext),
    /// A real message; `index` addresses the input slice, `dup` counts
    /// identical rows folded into this one (0 = not duplicated).
    History { index: usize, dup: usize },
    /// A provenance/model boundary marker (`agent @ node`, `model: X`).
    Marker { label: String, at: f64 },
}

/// What `build_rows` reads off a message — `HistoryMessageDto` maps to
/// this.
#[derive(Clone, Debug)]
pub struct RowInput<'a> {
    /// `"system" | "user" | "assistant" | "tool"`.
    pub role: &'a str,
    pub content: &'a str,
    /// Epoch milliseconds.
    pub created_at: f64,
    pub model: Option<&'a str>,
    /// Structured blocks — dedupe needs value equality; empty blocks
    /// can't carry attachments.
    pub blocks: Option<&'a [Value]>,
}

/// `hasAttachments` — true when a block list carries something
/// `content` cannot show (any non-`text` block).
fn has_attachments(blocks: Option<&[Value]>) -> bool {
    blocks.is_some_and(|blocks| {
        blocks
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) != Some("text"))
    })
}

/// `agent @ node` — the marker label; node may be empty on hand-written
/// spans.
fn span_label(span: &RunSpan) -> String {
    if span.node.is_empty() {
        span.agent.clone()
    } else {
        format!("{} @ {}", span.agent, span.node)
    }
}

/// Marks which agent+node ran each transcript segment. A span at/before
/// the first message heads the transcript — its marker goes on top so
/// every segment is labeled; each later span's marker sits before the
/// first message at/after its `at` (a span with no messages yet — e.g.
/// the current attach — trails the backlog and labels upcoming live
/// rows). A span that starts mid-transcript — a transfer-in — marks
/// inline at its boundary even when it's the only span recorded. The
/// one case with no marker: a single span covering the whole transcript
/// — the session ran end-to-end on one node+agent, which the header
/// already states.
fn insert_span_markers(
    rows: &mut Vec<ChatRow>,
    conversation: &[usize],
    messages: &[RowInput<'_>],
    spans: &[RunSpan],
) {
    let mut ordered: Vec<&RunSpan> = spans.iter().collect();
    ordered.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let first_at = conversation.first().map(|&i| messages[i].created_at);
    let heads_all = !ordered.is_empty() && first_at.is_none_or(|at| ordered[0].at <= at);
    if heads_all && ordered.len() == 1 && !conversation.is_empty() {
        for &index in conversation {
            rows.push(ChatRow::History { index, dup: 0 });
        }
        return;
    }
    let inline: &[&RunSpan] = if heads_all {
        &ordered[1..]
    } else {
        &ordered[..]
    };
    if heads_all {
        let first = ordered[0];
        rows.push(ChatRow::Marker {
            label: span_label(first),
            at: first.at,
        });
    }
    let mut next = 0;
    for &index in conversation {
        let at = messages[index].created_at;
        while next < inline.len() && at >= inline[next].at {
            let span = inline[next];
            rows.push(ChatRow::Marker {
                label: span_label(span),
                at: span.at,
            });
            next += 1;
        }
        rows.push(ChatRow::History { index, dup: 0 });
    }
    for span in &inline[next..] {
        rows.push(ChatRow::Marker {
            label: span_label(span),
            at: span.at,
        });
    }
}

/// Model ranges (the same idea as node/agent spans): an assistant
/// message's `model` marks which model generated it, so a transition
/// means the run switched mid-session — delimit it with a marker before
/// the first message under the new model. The first model is the
/// session default, not a boundary, so it earns no marker.
fn insert_model_markers(rows: &mut Vec<ChatRow>, messages: &[RowInput<'_>]) {
    let mut current: Option<&str> = None;
    let mut index = 0;
    while index < rows.len() {
        let model = match &rows[index] {
            ChatRow::History { index, .. } if messages[*index].role == "assistant" => {
                messages[*index].model
            }
            _ => None,
        };
        if let Some(model) = model {
            if current.is_some_and(|c| c != model) {
                let at = match &rows[index] {
                    ChatRow::History { index, .. } => messages[*index].created_at,
                    _ => 0.0,
                };
                rows.insert(
                    index,
                    ChatRow::Marker {
                        label: format!("model: {model}"),
                        at,
                    },
                );
                index += 1;
            }
            current = Some(model);
        }
        index += 1;
    }
}

/// All system nodes roll up into one context card at the top — devin
/// emits them per-turn, so positional runs would scatter the cards.
/// Devin also rewrites the context block per internal turn — the same
/// user prompt (and sometimes the reply) lands N times with only system
/// nodes in between — back-to-back duplicates in conversation order
/// fold into the kept row's `dup` count (the TS app dropped them
/// silently; the count feeds the UI's "N identical" fold).
pub fn build_rows(messages: &[RowInput<'_>], spans: &[RunSpan]) -> Vec<ChatRow> {
    let mut system_parts: Vec<(String, f64)> = Vec::new();
    let mut conversation: Vec<usize> = Vec::new();
    let mut dups: Vec<usize> = Vec::new(); // parallel to conversation
    for (index, m) in messages.iter().enumerate() {
        if m.role == "system" {
            system_parts.push((m.content.to_string(), m.created_at));
            continue;
        }
        // Empty assistant nodes carry the turn's tool_calls in the IR —
        // the calls themselves surface as `tool` rows, so a blank bubble
        // is pure noise. A message whose only payload is an attachment
        // is not blank, though.
        if m.role == "assistant" && m.content.trim().is_empty() && !has_attachments(m.blocks) {
            continue;
        }
        if let Some(&prev) = conversation.last() {
            let p = &messages[prev];
            // Two identical texts that carry different attachments are
            // not the same turn — devin rewrites prompt text verbatim,
            // attachments included.
            if p.role == m.role && p.content == m.content && p.blocks == m.blocks {
                if let Some(dup) = dups.last_mut() {
                    *dup += 1;
                }
                continue;
            }
        }
        conversation.push(index);
        dups.push(0);
    }
    let context = parse_system_context(&system_parts);
    let mut rows: Vec<ChatRow> = if context.is_empty() {
        Vec::new()
    } else {
        vec![ChatRow::Context(context)]
    };
    if spans.is_empty() {
        for &index in &conversation {
            rows.push(ChatRow::History { index, dup: 0 });
        }
    } else {
        insert_span_markers(&mut rows, &conversation, messages, spans);
    }
    // Runs after the span pass so a node/agent boundary + model change
    // on the same message stack both markers ahead of it (span first,
    // then model).
    insert_model_markers(&mut rows, messages);
    // Fold the dedupe counts into the history rows they collapsed into.
    let mut seen = 0usize;
    for row in &mut rows {
        if let ChatRow::History { dup, .. } = row {
            *dup = dups.get(seen).copied().unwrap_or(0);
            seen += 1;
        }
    }
    rows
}

/* ==== usage formatting ================================================= */

/// `Intl.NumberFormat` compact — `1.2k`, `3.4M`, `500`.
pub fn compact_number(n: f64) -> String {
    let compact = |v: f64, suffix: &str| {
        let s = format!("{v:.1}");
        format!("{}{}", s.trim_end_matches(".0"), suffix)
    };
    if n >= 1e9 {
        compact(n / 1e9, "B")
    } else if n >= 1e6 {
        compact(n / 1e6, "M")
    } else if n >= 1e3 {
        compact(n / 1e3, "k")
    } else {
        format!("{n:.0}")
    }
}

/// Grouped digits — `12,345` (en-US `Intl.NumberFormat`).
fn grouped_number(n: f64) -> String {
    let int = n.trunc().abs() as u64;
    let digits = int.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

/// `"$0.02"` — `""` when the store recorded no (or a non-positive) cost.
pub fn format_cost(cost: f64) -> String {
    if !cost.is_finite() || cost <= 0.0 {
        return String::new();
    }
    if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}

/// Compact footer form — `"↑ 1.2k  ↓ 340 tok"`, plus `"$0.02"` when
/// priced. Two plain spaces separate the segments (the footer renders
/// whitespace-pre); no middot separators.
pub fn usage_label(usage: &TokenUsage) -> String {
    let base = format!(
        "↑ {}  ↓ {} tok",
        compact_number(usage.input),
        compact_number(usage.output)
    );
    let cost = usage.cost.map_or_else(String::new, format_cost);
    if cost.is_empty() {
        base
    } else {
        format!("{base}  {cost}")
    }
}

/// Hover-title breakdown — every tier the agent recorded, grouped
/// digits.
pub fn format_usage(usage: &TokenUsage) -> String {
    let mut parts = vec![
        format!("{} input", grouped_number(usage.input)),
        format!("{} output", grouped_number(usage.output)),
    ];
    if usage.cache_read.is_some_and(|v| v > 0.0) {
        parts.push(format!(
            "{} cache read",
            grouped_number(usage.cache_read.unwrap_or(0.0))
        ));
    }
    if usage.cache_write.is_some_and(|v| v > 0.0) {
        parts.push(format!(
            "{} cache write",
            grouped_number(usage.cache_write.unwrap_or(0.0))
        ));
    }
    if usage.thinking.is_some_and(|v| v > 0.0) {
        parts.push(format!(
            "{} thinking",
            grouped_number(usage.thinking.unwrap_or(0.0))
        ));
    }
    if let Some(cost) = usage.cost {
        let cost = format_cost(cost);
        if !cost.is_empty() {
            parts.push(cost);
        }
    }
    parts.join(" · ")
}

/// `"950ms"` / `"1.2s"` / `"1m 4s"` — the parenthesized suffix on a
/// tool-call marker.
pub fn format_duration(ms: f64) -> String {
    if !ms.is_finite() || ms < 0.0 {
        return String::new();
    }
    if ms < 1000.0 {
        return format!("{ms:.0}ms");
    }
    if ms < 60_000.0 {
        return format!("{:.1}s", ms / 1000.0);
    }
    format!(
        "{}m {}s",
        (ms / 60_000.0) as u64,
        ((ms % 60_000.0) / 1000.0) as u64
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn row<'a>(role: &'a str, content: &'a str, created_at: f64) -> RowInput<'a> {
        RowInput {
            role,
            content,
            created_at,
            model: None,
            blocks: None,
        }
    }

    /* ---- tool display ------------------------------------------------ */

    #[test]
    fn categorize_maps_name_shapes() {
        assert_eq!(categorize("run_command"), ToolCategory::Exec);
        assert_eq!(categorize("Bash"), ToolCategory::Exec);
        assert_eq!(categorize("get_output"), ToolCategory::Exec);
        assert_eq!(categorize("edit_file"), ToolCategory::Edit);
        assert_eq!(categorize("write_file"), ToolCategory::Edit);
        assert_eq!(categorize("read_file"), ToolCategory::Read);
        assert_eq!(categorize("grep"), ToolCategory::Search);
        assert_eq!(categorize("fetch_url"), ToolCategory::Fetch);
        assert_eq!(categorize("todo"), ToolCategory::Todo);
        // `todowrite` contains "write" — the edit pattern wins first,
        // matching the TS order.
        assert_eq!(categorize("todo_write"), ToolCategory::Edit);
        assert_eq!(categorize("analyze"), ToolCategory::Other);
    }

    #[test]
    fn strip_ansi_removes_escapes() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("a\x1b]0;title\x07b"), "ab");
        assert_eq!(strip_ansi("none"), "none");
    }

    #[test]
    fn split_leading_json_reads_concatenated_snapshots() {
        let (values, rest) = split_leading_json(r#"{"a":1}{"b":2}tail"#);
        assert_eq!(values.len(), 2);
        assert_eq!(rest, "tail");
        // A partial trailing snapshot lands in rest.
        let (values, rest) = split_leading_json(r#"{"a":1}{"b":"#);
        assert_eq!(values.len(), 1);
        assert_eq!(rest, r#"{"b":"#);
    }

    #[test]
    fn exec_display_runs_command_with_exit_note() {
        let d = tool_summary(
            "run_command",
            Some(r#"{"command":"cargo test","timeout":30}"#),
            "Output from command in shell 4:\n42 passed\nExit code: 0",
            Some(0),
        );
        assert_eq!(d.category, ToolCategory::Exec);
        assert_eq!(d.label, "Ran command");
        assert_eq!(d.detail.as_deref(), Some("cargo test"));
        assert!(matches!(&d.segments[0], ToolSegment::Command(c) if c == "cargo test"));
        assert!(matches!(&d.segments[1], ToolSegment::Code(c) if c.contains("42 passed")));
        assert!(matches!(
            d.segments.last(),
            Some(ToolSegment::Note { text, error: false }) if text == "exit 0"
        ));
    }

    #[test]
    fn exec_display_flags_nonzero_exit() {
        let d = tool_summary("exec", None, "boom\nExit code: 1", None);
        assert!(matches!(
            d.segments.last(),
            Some(ToolSegment::Note { text, error: true }) if text == "exit 1"
        ));
    }

    #[test]
    fn read_display_unwraps_file_view() {
        let content = "<file-view path=\"/src/lib.rs\" start_line=\"10\" end_line=\"12\">\n10|fn a() {}\n11|fn b() {}\n</file-view>";
        let d = tool_summary(
            "read_file",
            Some(r#"{"file_path":"/src/lib.rs"}"#),
            content,
            None,
        );
        assert_eq!(d.label, "Read file");
        assert_eq!(d.detail.as_deref(), Some("/src/lib.rs:10–12"));
        assert!(
            matches!(&d.segments[0], ToolSegment::Code(c) if c.contains("fn a()") && !c.contains("10|"))
        );
    }

    #[test]
    fn edit_display_labels_and_diffs_from_args() {
        let d = tool_summary(
            "edit_file",
            Some(r#"{"file_path":"/a.rs","old_string":"let x = 1;","new_string":"let x = 2;"}"#),
            "The file /a.rs has been updated.\n\nedited file:\nlet x = 2;",
            None,
        );
        assert_eq!(d.label, "Edited file");
        assert_eq!(d.detail.as_deref(), Some("/a.rs"));
        // The `edited file:` envelope wins over the args diff.
        assert!(matches!(&d.segments[0], ToolSegment::Code(c) if c.contains("let x = 2;")));

        let d = tool_summary(
            "write_file",
            Some(r#"{"path":"/n.rs","content":"hi"}"#),
            "",
            None,
        );
        assert_eq!(d.label, "Wrote file");
        assert!(matches!(&d.segments[0], ToolSegment::Diff(t) if t.contains("+ hi")));
    }

    #[test]
    fn search_and_fetch_display() {
        let d = tool_summary(
            "grep",
            Some(r#"{"pattern":"fn main"}"#),
            "Found 2 match(es) for pattern 'fn main' in src:\nsrc/a.rs:1: fn main() {}",
            None,
        );
        assert_eq!(d.label, "Searched codebase");
        assert_eq!(d.detail.as_deref(), Some("fn main"));
        assert!(
            matches!(&d.segments[0], ToolSegment::Note { text, .. } if text == "2 match(es) in src")
        );

        let d = tool_summary(
            "web_fetch",
            Some(r#"{"url":"https://x.dev"}"#),
            "# Page\nbody",
            None,
        );
        assert_eq!(d.label, "Fetched web content");
        assert_eq!(d.detail.as_deref(), Some("https://x.dev"));
    }

    #[test]
    fn generic_display_prettifies_name() {
        let d = tool_summary("analyze_deps", None, "done", None);
        assert_eq!(d.label, "Analyze deps");
        let d = tool_summary("todo", None, "", None);
        assert_eq!(d.label, "Updated todos");
    }

    /* ---- file diffs ---------------------------------------------------- */

    #[test]
    fn file_diff_view_elides_unchanged_edges() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj";
        let new = "a\nb\nc\nd\ne\nf\ng\nh\ni\nX";
        let v = file_diff_view(&ToolCallDiff {
            path: "/f".into(),
            old_text: Some(old.into()),
            new_text: Some(new.into()),
        });
        assert_eq!(v.removed, 1);
        assert_eq!(v.added, 1);
        assert!(v.text.contains("⋮ 9 unchanged lines"));
        assert!(v.text.contains("- j"));
        assert!(v.text.contains("+ X"));
        // Context lines around the hunk.
        assert!(v.text.contains("  i"));
    }

    #[test]
    fn file_diff_view_create_has_only_adds() {
        let v = file_diff_view(&ToolCallDiff {
            path: "/new".into(),
            old_text: None,
            new_text: Some("a\nb".into()),
        });
        assert_eq!(v.added, 2);
        assert_eq!(v.removed, 0);
        assert!(v.text.contains("+ a"));
    }

    #[test]
    fn tool_content_segments_handles_terminal_and_text() {
        let segs = tool_content_segments(&[
            json!({"type": "terminal", "terminalId": "t7", "output": "hi\n"}),
            json!({"type": "text", "text": "blob"}),
            json!({"type": "content", "content": {"type": "text", "text": "inner"}}),
        ]);
        assert!(matches!(&segs[0], ToolSegment::Note { text, .. } if text == "Terminal t7"));
        assert!(matches!(&segs[1], ToolSegment::Code(c) if c == "hi\n"));
        assert!(matches!(&segs[2], ToolSegment::Code(c) if c == "blob"));
        assert!(matches!(&segs[3], ToolSegment::Code(c) if c == "inner"));
    }

    /* ---- system context ------------------------------------------------ */

    #[test]
    fn parse_system_context_reads_tagged_sections() {
        let msgs = vec![
            (
                "<system_info>\nThe following information is automatically generated context.\nCurrent workspace directories:\n  /work/messy (cwd)\n\nPlatform: linux\n</system_info>"
                    .to_string(),
                1.0,
            ),
            (
                "<rules type=\"always-on\" path=\"/home/luis/.codeium/windsurf/memories/global_rules.md\">\n<rule name=\"global_rules\" path=\"/x/global_rules.md\">\nbe nice\n</rule>\n</rules>"
                    .to_string(),
                1.0,
            ),
            (
                "<available_skills>\n- **deploy**: ship it (source: skills/deploy/SKILL.md)\n</available_skills>"
                    .to_string(),
                1.0,
            ),
            ("You are Devin, an AI pair programmer.".to_string(), 1.0),
        ];
        let ctx = parse_system_context(&msgs);
        assert_eq!(ctx.workspaces, vec!["/work/messy"]);
        assert_eq!(ctx.platform.as_deref(), Some("linux"));
        assert_eq!(ctx.rules.len(), 1);
        assert_eq!(ctx.rules[0].name, "global_rules");
        assert_eq!(ctx.rules[0].content.as_deref(), Some("be nice"));
        assert_eq!(ctx.skills.len(), 1);
        assert_eq!(ctx.skills[0].name, "deploy");
        assert_eq!(
            ctx.skills[0].source.as_deref(),
            Some("skills/deploy/SKILL.md")
        );
        assert_eq!(ctx.prompt_text, "You are Devin, an AI pair programmer.");
        assert!(!ctx.is_empty());
    }

    #[test]
    fn parse_system_context_splits_prompt_headings_and_reports() {
        let msgs = vec![
            (
                "Intro.\n# Modes\ncode mode only\n# Style\nbe terse".to_string(),
                1.0,
            ),
            (
                "<subagent_completion_notification>[Background subagent with agent_id=abc completed]\n# Scan results\nfound 3 issues</subagent_completion_notification>"
                    .to_string(),
                5.0,
            ),
        ];
        let ctx = parse_system_context(&msgs);
        assert_eq!(ctx.reports.len(), 1);
        assert_eq!(ctx.reports[0].agent_id.as_deref(), Some("abc"));
        assert_eq!(ctx.reports[0].title.as_deref(), Some("Scan results"));
        assert_eq!(ctx.reports[0].body, "found 3 issues");
        assert_eq!(ctx.prompt_sections.len(), 2);
        assert_eq!(ctx.prompt_sections[0].title, "Modes");
    }

    /* ---- chat rows ----------------------------------------------------- */

    #[test]
    fn build_rows_folds_dupes_and_system() {
        let msgs = vec![
            row("system", "<system_info>Platform: linux</system_info>", 1.0),
            row("user", "hi", 2.0),
            row("assistant", "on it", 3.0),
            row("assistant", "on it", 4.0),
            row("assistant", "on it", 5.0),
            row("assistant", "", 6.0), // empty carrier — dropped
        ];
        let rows = build_rows(&msgs, &[]);
        assert!(matches!(&rows[0], ChatRow::Context(_)));
        let history: Vec<&ChatRow> = rows
            .iter()
            .filter(|r| matches!(r, ChatRow::History { .. }))
            .collect();
        assert_eq!(history.len(), 2);
        assert!(matches!(history[1], ChatRow::History { index: 2, dup: 2 }));
    }

    #[test]
    fn build_rows_span_markers() {
        let msgs = vec![
            row("user", "a", 10.0),
            row("assistant", "b", 20.0),
            row("user", "c", 30.0),
        ];
        // One span covering the whole transcript → no markers.
        let rows = build_rows(
            &msgs,
            &[RunSpan {
                at: 1.0,
                agent: "devin".into(),
                node: "n1".into(),
            }],
        );
        assert!(!rows.iter().any(|r| matches!(r, ChatRow::Marker { .. })));
        // A transfer-in mid-transcript marks inline.
        let rows = build_rows(
            &msgs,
            &[RunSpan {
                at: 25.0,
                agent: "claude".into(),
                node: "tower".into(),
            }],
        );
        let marker = rows
            .iter()
            .position(|r| matches!(r, ChatRow::Marker { label, .. } if label == "claude @ tower"))
            .unwrap();
        // Before the "c" row.
        assert!(matches!(&rows[marker + 1], ChatRow::History { index, .. } if *index == 2));
        // Two spans: first heads the transcript, second sits inline.
        let rows = build_rows(
            &msgs,
            &[
                RunSpan {
                    at: 1.0,
                    agent: "devin".into(),
                    node: "n1".into(),
                },
                RunSpan {
                    at: 25.0,
                    agent: "devin".into(),
                    node: "tower".into(),
                },
            ],
        );
        assert!(matches!(&rows[0], ChatRow::Marker { label, .. } if label == "devin @ n1"));
        assert!(
            rows.iter()
                .any(|r| matches!(r, ChatRow::Marker { label, .. } if label == "devin @ tower"))
        );
    }

    #[test]
    fn build_rows_model_markers() {
        let mut a = row("assistant", "one", 1.0);
        a.model = Some("m1");
        let mut b = row("assistant", "two", 2.0);
        b.model = Some("m2");
        let msgs = vec![a, b];
        let rows = build_rows(&msgs, &[]);
        assert!(matches!(
            &rows[1],
            ChatRow::Marker { label, .. } if label == "model: m2"
        ));
    }

    /* ---- usage ---------------------------------------------------------- */

    #[test]
    fn usage_formats() {
        let u = TokenUsage {
            input: 4200.0,
            output: 88.0,
            cache_read: Some(1024.0),
            cache_write: None,
            thinking: None,
            cost: Some(0.013),
        };
        assert_eq!(compact_number(4200.0), "4.2k");
        assert_eq!(compact_number(88.0), "88");
        assert_eq!(format_cost(0.013), "$0.01");
        assert_eq!(format_cost(0.004), "$0.0040");
        assert_eq!(format_cost(0.0), "");
        assert_eq!(usage_label(&u), "↑ 4.2k  ↓ 88 tok  $0.01");
        assert_eq!(
            format_usage(&u),
            "4,200 input · 88 output · 1,024 cache read · $0.01"
        );
        let free = TokenUsage {
            input: 10.0,
            output: 5.0,
            ..TokenUsage::default()
        };
        assert_eq!(usage_label(&free), "↑ 10  ↓ 5 tok");
    }
}
