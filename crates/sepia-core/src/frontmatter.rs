//! Minimal YAML-frontmatter reader/writer for agent config files —
//! `---\n<yaml>\n---\n<body>` markdown.
//!
//! Deliberate subset, not a YAML implementation: scalar keys, `- item`
//! lists, `|`/`>` block scalars, one level of nested maps, `[a, b]` flow
//! lists; anything else stays a raw string so callers pass it through
//! `metadata`.

use serde_json::{Map, Value, json};

pub struct FrontmatterDoc {
    pub attributes: Map<String, Value>,
    pub body: String,
}

/// `/^---[ \t]*\r?\n/` — `---` on its own first line followed by a newline.
pub fn has(text: &str) -> bool {
    let Some(first) = text.split('\n').next() else {
        return false;
    };
    if text.split('\n').count() < 2 {
        return false;
    }
    first.trim_end_matches([' ', '\t', '\r']) == "---"
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn parse_scalar(raw: &str) -> Value {
    let s = raw.trim();
    match s {
        "" => return Value::String(String::new()),
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" | "~" => return Value::Null,
        _ => {}
    }
    if let Ok(n) = s.parse::<i64>() {
        return json!(n);
    }
    // `-?\d+(\.\d+)?` — optional sign, digits, optional `.digits`.
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    let numeric = !unsigned.is_empty()
        && unsigned
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && unsigned.matches('.').count() <= 1;
    if numeric
        && unsigned.contains('.')
        && let Ok(n) = s.parse::<f64>()
    {
        return json!(n);
    }
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_string)
            .unwrap_or_else(|| json!(s[1..s.len() - 1]));
    }
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return json!(s[1..s.len() - 1].replace("''", "'"));
    }
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return Value::Array(Vec::new());
        }
        return Value::Array(inner.split(',').map(parse_scalar).collect());
    }
    json!(s)
}

/// Index of the next non-blank line at or after `i`, or `lines.len()`.
fn next_content(lines: &[&str], i: usize) -> usize {
    let mut j = i;
    while j < lines.len() && is_blank(lines[j]) {
        j += 1;
    }
    j
}

/// Collect lines strictly more indented than `indent` (or blank).
fn collect_children<'a>(lines: &[&'a str], i: usize, indent: usize) -> (Vec<&'a str>, usize) {
    let mut out = Vec::new();
    let mut j = i;
    while j < lines.len() {
        let line = lines[j];
        if !is_blank(line) && indent_of(line) <= indent {
            break;
        }
        out.push(line);
        j += 1;
    }
    let mut start = 0;
    let mut end = out.len();
    while start < end && is_blank(out[start]) {
        start += 1;
    }
    while end > start && is_blank(out[end - 1]) {
        end -= 1;
    }
    (out[start..end].to_vec(), j)
}

fn dedent(lines: &[&str]) -> Vec<String> {
    let mut min = usize::MAX;
    for line in lines {
        if is_blank(line) {
            continue;
        }
        min = min.min(indent_of(line));
    }
    if min == usize::MAX || min == 0 {
        return lines.iter().map(ToString::to_string).collect();
    }
    lines
        .iter()
        .map(|line| {
            if is_blank(line) {
                String::new()
            } else {
                line[min..].to_string()
            }
        })
        .collect()
}

fn parse_node(lines: &[&str], i: usize, indent: usize) -> (Value, usize) {
    let first = next_content(lines, i);
    if first >= lines.len() {
        return (json!({}), first);
    }
    let trimmed = lines[first].trim();
    if trimmed.starts_with("- ") || trimmed == "-" {
        return parse_list(lines, first, indent_of(lines[first]));
    }
    parse_map(lines, first, indent)
}

fn parse_list(lines: &[&str], i: usize, indent: usize) -> (Value, usize) {
    let mut items = Vec::new();
    let mut j = i;
    while j < lines.len() {
        let line = lines[j];
        if is_blank(line) {
            j += 1;
            continue;
        }
        if indent_of(line) != indent || !line.trim_start().starts_with('-') {
            break;
        }
        let after_dash = line.trim_start()[1..].trim();
        if after_dash.is_empty() {
            let (child_lines, next) = collect_children(lines, j + 1, indent);
            if child_lines.is_empty() {
                items.push(Value::Null);
            } else {
                let dedented: Vec<String> = dedent(&child_lines);
                let refs: Vec<&str> = dedented.iter().map(String::as_str).collect();
                items.push(parse_node(&refs, 0, 0).0);
            }
            j = next;
            continue;
        }
        items.push(parse_scalar(after_dash));
        j += 1;
    }
    (Value::Array(items), j)
}

/// `^([^\s:#][^:]*):[ \t]*(.*)$`
fn key_line(line: &str) -> Option<(String, String)> {
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty() || key.starts_with(char::is_whitespace) || key.starts_with('#') {
        return None;
    }
    let rest = line[colon + 1..].trim_start_matches([' ', '\t']);
    Some((key.trim().to_string(), rest.to_string()))
}

fn parse_map(lines: &[&str], i: usize, indent: usize) -> (Value, usize) {
    let mut map = Map::new();
    let mut j = i;
    while j < lines.len() {
        let line = lines[j];
        if is_blank(line) {
            j += 1;
            continue;
        }
        if indent_of(line) != indent {
            break;
        }
        let Some((key, rest)) = key_line(line) else {
            break;
        };
        if rest.is_empty() {
            let probe = next_content(lines, j + 1);
            if probe < lines.len() && indent_of(lines[probe]) > indent {
                let (child_lines, next) = collect_children(lines, j + 1, indent);
                let dedented: Vec<String> = dedent(&child_lines);
                let refs: Vec<&str> = dedented.iter().map(String::as_str).collect();
                map.insert(key, parse_node(&refs, 0, 0).0);
                j = next;
            } else {
                map.insert(key, Value::Null);
                j += 1;
            }
            continue;
        }
        let trimmed = rest.trim();
        let block = trimmed.strip_prefix(['|', '>']).and_then(|tail| {
            if tail.is_empty() || tail == "+" || tail == "-" {
                Some(trimmed.chars().next().unwrap_or('|'))
            } else {
                None
            }
        });
        if let Some(marker) = block {
            let (child_lines, next) = collect_children(lines, j + 1, indent);
            let flat = dedent(&child_lines);
            if marker == '>' {
                let joined = flat.join(" ");
                let mut folded = String::with_capacity(joined.len());
                let mut prev_ws = false;
                for c in joined.chars() {
                    if c == ' ' || c == '\t' {
                        if !prev_ws {
                            folded.push(' ');
                        }
                        prev_ws = true;
                    } else {
                        folded.push(c);
                        prev_ws = false;
                    }
                }
                map.insert(key, json!(folded.trim()));
            } else {
                map.insert(key, json!(flat.join("\n")));
            }
            j = next;
            continue;
        }
        map.insert(key, parse_scalar(&rest));
        j += 1;
    }
    (Value::Object(map), j)
}

/// Split `text` into frontmatter attributes and body. No `---` fence means
/// empty attributes and the whole text as body.
pub fn parse(text: &str) -> FrontmatterDoc {
    if !has(text) {
        return FrontmatterDoc {
            attributes: Map::new(),
            body: text.to_string(),
        };
    }
    // TS splits on /\r?\n/ — strip a trailing '\r' per line identically.
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut close = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim_end_matches([' ', '\t']) == "---" {
            close = Some(i);
            break;
        }
        if !is_blank(line) && indent_of(line) == 0 && key_line(line).is_none() {
            break;
        }
    }
    let Some(close) = close else {
        return FrontmatterDoc {
            attributes: Map::new(),
            body: text.to_string(),
        };
    };
    let yaml: Vec<&str> = lines[1..close].to_vec();
    let body = lines[close + 1..]
        .join("\n")
        .trim_start_matches('\n')
        .to_string();
    let (parsed, _) = parse_node(&yaml, 0, 0);
    let attributes = match parsed {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    FrontmatterDoc { attributes, body }
}

fn needs_quote(s: &str) -> bool {
    if s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) || s.contains('\n') {
        return true;
    }
    if s.chars().any(|c| ":#'\"[]{}&*!|>@`%,".contains(c)) {
        return true;
    }
    let lower = s.to_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
    ) {
        return true;
    }
    s.parse::<f64>().is_ok() && !s.is_empty()
}

/// YAML-safe scalar: plain when unambiguous, JSON-quoted otherwise.
fn scalar_out(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => {
            let s = match other {
                Value::String(s) => s.clone(),
                o => serde_json::to_string(o).unwrap_or_default(),
            };
            if s.contains('\n') || needs_quote(&s) {
                serde_json::to_string(&s).unwrap_or_else(|_| format!("\"{s}\""))
            } else {
                s
            }
        }
    }
}

fn render_entry(key: &str, value: &Value, indent: &str) -> Vec<String> {
    match value {
        Value::Array(items) if items.is_empty() => vec![format!("{indent}{key}: []")],
        Value::Array(items) => {
            let mut out = vec![format!("{indent}{key}:")];
            out.extend(
                items
                    .iter()
                    .map(|item| format!("{indent}  - {}", scalar_out(item))),
            );
            out
        }
        Value::Object(map) => {
            let mut out = vec![format!("{indent}{key}:")];
            out.extend(render_map(map, &format!("{indent}  ")));
            out
        }
        Value::String(s) if s.contains('\n') => {
            let mut out = vec![format!("{indent}{key}: |")];
            out.extend(s.split('\n').map(|line| format!("{indent}  {line}")));
            out
        }
        other => vec![format!("{indent}{key}: {}", scalar_out(other))],
    }
}

fn render_map(attrs: &Map<String, Value>, indent: &str) -> Vec<String> {
    attrs
        .iter()
        .flat_map(|(key, value)| render_entry(key, value, indent))
        .collect()
}

/// Rebuild the markdown file: a `---` fence when attributes is non-empty
/// (order preserved), then the body verbatim.
pub fn render(attributes: &Map<String, Value>, body: &str) -> String {
    if attributes.is_empty() {
        return if body.ends_with('\n') {
            body.to_string()
        } else {
            format!("{body}\n")
        };
    }
    let yaml = render_map(attributes, "");
    let trimmed_body = body.trim_start_matches('\n');
    format!("---\n{}\n---\n\n{trimmed_body}", yaml.join("\n"))
}
