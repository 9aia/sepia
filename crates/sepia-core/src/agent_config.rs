//! Agent-config IR — the portable layer over the four agents' on-disk
//! configuration: skills, rules, commands, hooks and subagent definitions.
//! Pure domain only — filesystem walkers live in `sepia-driver-sdk`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSkill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub files: Vec<ConfigFile>,
    #[serde(default)]
    pub metadata: Value,
}

/// `kind: "instructions"` = the agent's single always-loaded memory file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    Rule,
    Instructions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub body: String,
    /// Glob patterns the rule applies to. Empty means unscoped.
    #[serde(default)]
    pub globs: Vec<String>,
    #[serde(default)]
    pub always_apply: bool,
    #[serde(default = "default_rule_kind")]
    pub kind: RuleKind,
    #[serde(default)]
    pub metadata: Value,
}

fn default_rule_kind() -> RuleKind {
    RuleKind::Rule
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The prompt template the slash command injects.
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigAgent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The subagent's system prompt.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    Command,
    Prompt,
}

fn default_hook_kind() -> HookKind {
    HookKind::Command
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHook {
    /// Canonical PascalCase event name (`PreToolUse`, …).
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(default = "default_hook_kind", rename = "type")]
    pub kind: HookKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_sec: Option<f64>,
    /// Cursor-only: a crashing hook blocks the action instead of passing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_closed: Option<bool>,
    /// Cursor `loop_limit` on stop/subagentStop follow-up loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_limit: Option<f64>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    #[serde(default)]
    pub skills: Vec<ConfigSkill>,
    #[serde(default)]
    pub rules: Vec<ConfigRule>,
    #[serde(default)]
    pub commands: Vec<ConfigCommand>,
    #[serde(default)]
    pub hooks: Vec<ConfigHook>,
    #[serde(default)]
    pub agents: Vec<ConfigAgent>,
    #[serde(default)]
    pub mcp_servers: Map<String, Value>,
    #[serde(default)]
    pub metadata: Value,
}

/// Canonical hook events keyed canonical → Cursor wire name. Claude and
/// Devin share the PascalCase schema, so canonical doubles as their wire name.
fn cursor_hook_events() -> &'static [(&'static str, &'static str)] {
    &[
        ("SessionStart", "sessionStart"),
        ("SessionEnd", "sessionEnd"),
        ("UserPromptSubmit", "beforeSubmitPrompt"),
        ("PreToolUse", "preToolUse"),
        ("PostToolUse", "postToolUse"),
        ("PostToolUseFailure", "postToolUseFailure"),
        ("SubagentStart", "subagentStart"),
        ("SubagentStop", "subagentStop"),
        ("Stop", "stop"),
        ("PreCompact", "preCompact"),
        ("Notification", "afterAgentResponse"),
        ("BeforeShellExecution", "beforeShellExecution"),
        ("AfterShellExecution", "afterShellExecution"),
        ("BeforeMCPExecution", "beforeMCPExecution"),
        ("AfterMCPExecution", "afterMCPExecution"),
        ("BeforeReadFile", "beforeReadFile"),
        ("AfterFileEdit", "afterFileEdit"),
        ("BeforeTabFileRead", "beforeTabFileRead"),
        ("AfterTabFileEdit", "afterTabFileEdit"),
        ("AfterAgentThought", "afterAgentThought"),
    ]
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn decapitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

/// Cursor wire name → canonical (`beforeSubmitPrompt` → `UserPromptSubmit`).
pub fn canonical_hook_event(wire: &str) -> String {
    for (canonical, cursor) in cursor_hook_events() {
        if *cursor == wire {
            return (*canonical).to_string();
        }
    }
    if wire.chars().next().is_some_and(char::is_lowercase) {
        capitalize(wire)
    } else {
        wire.to_string()
    }
}

/// Canonical → Cursor wire name; unknown events fall back to camelCase.
pub fn cursor_hook_event(canonical: &str) -> String {
    for (c, cursor) in cursor_hook_events() {
        if *c == canonical {
            return (*cursor).to_string();
        }
    }
    decapitalize(canonical)
}

/// Canonical → Claude/Devin wire name (the schema is already PascalCase).
pub fn claude_hook_event(canonical: &str) -> String {
    if canonical.chars().next().is_some_and(char::is_lowercase) {
        capitalize(canonical)
    } else {
        canonical.to_string()
    }
}

// ---------------------------------------------------------------------------
// Wire JSON — `config export` output / `config import` input.
// ---------------------------------------------------------------------------

/// Serialize an `AgentConfig` to the `config export` JSON payload —
/// `version` first, then the item lists (`metadata` is IR-only).
pub fn config_to_json(config: &AgentConfig) -> Value {
    json!({
        "version": 1,
        "skills": config.skills,
        "rules": config.rules,
        "commands": config.commands,
        "hooks": config.hooks,
        "agents": config.agents,
        "mcpServers": config.mcp_servers,
    })
}

/// Decode a config JSON payload; errors on a non-IR shape.
pub fn config_from_json(input: &Value) -> Result<AgentConfig, serde_json::Error> {
    let mut config: AgentConfig = serde_json::from_value(input.clone())?;
    config.metadata = Value::Null;
    Ok(config)
}

// ---------------------------------------------------------------------------
// Shared adapter plumbing — pure parts only.
// ---------------------------------------------------------------------------

/// What a writer did to one file — the CLI prints these.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigWriteAction {
    pub path: String,
    pub action: WriteActionKind,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteActionKind {
    Wrote,
    Updated,
    Unchanged,
    Merged,
    Skipped,
}

/// Item names become path segments (`skills/<name>/`, `rules/<name>.md`), so
/// they must be a single safe stem. Returns the sanitized stem or `None`.
pub fn safe_file_stem(name: &str) -> Option<String> {
    let mut cleaned = name
        .trim()
        .replace(['/', '\\', '\0'], "-")
        .trim_start_matches('.')
        .to_string();
    // `\s+` → `-`
    let mut collapsed = String::with_capacity(cleaned.len());
    let mut pending_ws = false;
    for c in cleaned.chars() {
        if c.is_whitespace() {
            pending_ws = true;
        } else {
            if pending_ws && !collapsed.is_empty() {
                collapsed.push('-');
            }
            collapsed.push(c);
            pending_ws = false;
        }
    }
    cleaned = collapsed;
    for ext in [".md", ".mdc", ".MD", ".MDC"] {
        if cleaned.ends_with(ext) {
            cleaned.truncate(cleaned.len() - ext.len());
            break;
        }
    }
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    Some(cleaned)
}

pub fn is_object(value: &Value) -> bool {
    value.is_object()
}

/// `globs`/`tools`/`allowed-tools` accept a comma string or a list.
pub fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn str_or_none(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn num_or_none(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// Flatten the Claude/Devin hook schema into `ConfigHook`s. Both containers
/// share `{Event: [{matcher?, hooks: [{type, command?, prompt?, timeout?}]}]}`;
/// a `hooks` key wins when it is an object.
pub fn hooks_from_claude_json(settings: &Map<String, Value>, source: &str) -> Vec<ConfigHook> {
    let hooks = match settings.get("hooks") {
        Some(Value::Object(nested)) => nested,
        _ => settings,
    };
    let mut out = Vec::new();
    for (event, groups) in hooks {
        let Some(groups) = groups.as_array() else {
            continue;
        };
        for group in groups {
            let Some(group) = group.as_object() else {
                continue;
            };
            let matcher = str_or_none(group.get("matcher"));
            let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for entry in entries {
                let Some(entry) = entry.as_object() else {
                    continue;
                };
                out.push(ConfigHook {
                    event: claude_hook_event(event),
                    matcher: matcher.clone(),
                    kind: if entry.get("type").and_then(Value::as_str) == Some("prompt") {
                        HookKind::Prompt
                    } else {
                        HookKind::Command
                    },
                    command: str_or_none(entry.get("command")),
                    prompt: str_or_none(entry.get("prompt")),
                    timeout_sec: num_or_none(entry.get("timeout")),
                    fail_closed: None,
                    loop_limit: None,
                    metadata: json!({ "source": source }),
                });
            }
        }
    }
    out
}

fn claude_hook_entry(hook: &ConfigHook) -> Map<String, Value> {
    let mut entry = Map::new();
    entry.insert("type".into(), json!(hook.kind));
    if let Some(command) = &hook.command {
        entry.insert("command".into(), json!(command));
    }
    if let Some(prompt) = &hook.prompt {
        entry.insert("prompt".into(), json!(prompt));
    }
    if let Some(timeout) = hook.timeout_sec {
        entry.insert("timeout".into(), json!(timeout));
    }
    entry
}

/// Merge `hooks` into a bare event map — Devin's `hooks.v1.json` shape —
/// preserving existing events. Exact duplicates are not re-added.
pub fn merge_hook_events(
    existing: &Map<String, Value>,
    hooks: &[ConfigHook],
) -> Map<String, Value> {
    let mut merged = existing.clone();
    for hook in hooks {
        let event = claude_hook_event(&hook.event);
        let matcher = hook.matcher.clone().unwrap_or_default();
        let groups = merged
            .get(&event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let entry = Value::Object(claude_hook_entry(hook));
        let entry_key = serde_json::to_string(&entry).unwrap_or_default();
        let mut updated = groups.clone();
        // First group sharing the matcher wins — mirrors TS `.find()`.
        let mut handled = false;
        for group in &mut updated {
            let Value::Object(group) = group else {
                continue;
            };
            let group_matcher = str_or_none(group.get("matcher"));
            let want = if matcher.is_empty() {
                None
            } else {
                Some(matcher.clone())
            };
            if group_matcher != want {
                continue;
            }
            let entries = group
                .get("hooks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !entries
                .iter()
                .any(|e| serde_json::to_string(e).unwrap_or_default() == entry_key)
            {
                let mut next = entries;
                next.push(entry.clone());
                group.insert("hooks".into(), Value::Array(next));
            }
            handled = true;
            break;
        }
        if !handled {
            let mut group = Map::new();
            if !matcher.is_empty() {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert("hooks".into(), json!([entry]));
            updated.push(Value::Object(group));
        }
        merged.insert(event, Value::Array(updated));
    }
    merged
}

/// Merge `hooks` into a Claude settings object's `hooks` block.
pub fn merge_claude_hooks(
    settings: &Map<String, Value>,
    hooks: &[ConfigHook],
) -> Map<String, Value> {
    let existing = match settings.get("hooks") {
        Some(Value::Object(h)) => h.clone(),
        _ => Map::new(),
    };
    let mut out = settings.clone();
    out.insert(
        "hooks".into(),
        Value::Object(merge_hook_events(&existing, hooks)),
    );
    out
}

/// Marker pair wrapping the rules block sepia manages inside a memory file.
pub const RULES_BLOCK_BEGIN: &str = "<!-- sepia:rules -->";
pub const RULES_BLOCK_END: &str = "<!-- /sepia:rules -->";

fn render_rule(rule: &ConfigRule) -> String {
    let scope = if rule.globs.is_empty() {
        String::new()
    } else {
        format!("\n> Applies to: {}\n", rule.globs.join(", "))
    };
    let desc = rule
        .description
        .as_ref()
        .map_or(String::new(), |d| format!("\n> {d}\n"));
    format!("## {}\n{desc}{scope}\n{}\n", rule.name, rule.body.trim())
}

/// Merge `rules` into a memory file (`CLAUDE.md`, `AGENTS.md`): the IR's
/// rules live between the `sepia:rules` markers, replacing a previous
/// block and preserving whatever the file held outside it. `None` when
/// there is nothing to write.
pub fn rules_to_memory_file(existing: Option<&str>, rules: &[ConfigRule]) -> Option<String> {
    let text = existing.unwrap_or("");
    let begin = text.find(RULES_BLOCK_BEGIN);
    let end = text.find(RULES_BLOCK_END);
    let had_block = matches!((begin, end), (Some(b), Some(e)) if e > b);
    if rules.is_empty() && !had_block {
        return None;
    }
    let block = format!(
        "{RULES_BLOCK_BEGIN}\n\n{}\n{RULES_BLOCK_END}",
        rules.iter().map(render_rule).collect::<Vec<_>>().join("\n")
    );
    if !had_block {
        let base = text.trim_end();
        return Some(if base.is_empty() {
            format!("{block}\n")
        } else {
            format!("{base}\n\n{block}\n")
        });
    }
    let (begin, end) = (begin?, end?);
    let after = end + RULES_BLOCK_END.len();
    Some(format!("{}{block}{}", &text[..begin], &text[after..]))
}

/// Merge `mcpServers` into an MCP JSON object, preserving other keys.
pub fn merge_mcp_servers(
    existing: Option<&Map<String, Value>>,
    servers: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    if servers.is_empty() {
        return None;
    }
    let base = existing.cloned().unwrap_or_default();
    let current = match base.get("mcpServers") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    let mut merged = current;
    for (key, value) in servers {
        merged.insert(key.clone(), value.clone());
    }
    let mut out = base;
    out.insert("mcpServers".into(), Value::Object(merged));
    Some(out)
}

/// Frontmatter attributes a skill write emits: name/description + leftovers.
pub fn skill_attributes(skill: &ConfigSkill) -> Map<String, Value> {
    let meta = skill.metadata.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    out.insert("name".into(), json!(skill.name));
    if let Some(description) = &skill.description {
        out.insert("description".into(), json!(description));
    }
    for (key, value) in meta {
        if key == "sourcePath" || key == "source" {
            continue;
        }
        out.insert(key, value);
    }
    out
}
