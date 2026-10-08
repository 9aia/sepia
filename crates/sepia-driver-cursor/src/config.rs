//! `CursorConfig.ts` port — Cursor config store: `dir` is a `.cursor`
//! directory (`~/.cursor` user scope or `<repo>/.cursor` project scope).
//!
//! Read:
//! - `rules/*.{md,mdc}` — frontmatter `description`/`globs` (comma string or
//!   list)/`alwaysApply`; body is the rule text.
//! - `commands/<name>.md`, `agents/<name>.md`, `skills/<name>/SKILL.md`
//!   (the sibling `skills-cursor/` tree holds Cursor's built-ins and is not
//!   scanned).
//! - `hooks.json` `{version, hooks: {event: [entry]}}` — flat entries with
//!   `command`/`type`/`timeout`/`matcher`/`failClosed`/`loop_limit`; camelCase
//!   events normalize to the canonical PascalCase names.
//! - `mcp.json` `mcpServers`.
//!
//! Write is symmetric: rules land as `rules/<name>.md` (the modern `.md`
//! form — `.mdc` files still round-trip as `.md`), hooks merge into
//! `hooks.json` deduped per entry, MCP servers merge into `mcp.json`.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigHook, ConfigRule, ConfigWriteAction, HookKind,
    RuleKind, canonical_hook_event, cursor_hook_event, merge_mcp_servers, safe_file_stem,
    string_list,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver_sdk::fs as sdk_fs;
use serde_json::{Map, Value, json};

/// `metadata` minus the provenance keys read-side stamping adds.
fn extra_frontmatter(meta: &Value) -> Map<String, Value> {
    let mut rest = meta.as_object().cloned().unwrap_or_default();
    rest.shift_remove("sourcePath");
    rest.shift_remove("source");
    rest.shift_remove("sourceEvent");
    rest
}

fn doc_to_rule(doc: &sdk_fs::MarkdownDoc) -> ConfigRule {
    let mut rest = doc.attributes.clone();
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    let globs = rest
        .remove("globs")
        .map_or_else(Vec::new, |v| string_list(&v));
    let always_apply = rest
        .remove("alwaysApply")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigRule {
        name: doc.stem.clone(),
        description,
        body: doc.body.clone(),
        globs,
        always_apply,
        kind: RuleKind::Rule,
        metadata: Value::Object(rest),
    }
}

fn doc_to_command(doc: &sdk_fs::MarkdownDoc) -> ConfigCommand {
    let mut rest = doc.attributes.clone();
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    let argument_hint = rest
        .remove("argument-hint")
        .and_then(|v| v.as_str().map(str::to_string));
    let allowed_tools = rest
        .remove("allowed-tools")
        .map_or_else(Vec::new, |v| string_list(&v));
    let model = rest
        .remove("model")
        .and_then(|v| v.as_str().map(str::to_string));
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigCommand {
        name: doc.stem.clone(),
        description,
        body: doc.body.clone(),
        argument_hint,
        allowed_tools,
        model,
        metadata: Value::Object(rest),
    }
}

fn doc_to_agent(doc: &sdk_fs::MarkdownDoc) -> ConfigAgent {
    let mut rest = doc.attributes.clone();
    let name = rest
        .remove("name")
        .and_then(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string));
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    let tools = rest
        .remove("tools")
        .map_or_else(Vec::new, |v| string_list(&v));
    let model = rest
        .remove("model")
        .and_then(|v| v.as_str().map(str::to_string));
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigAgent {
        name: name.unwrap_or_else(|| doc.stem.clone()),
        description,
        body: doc.body.clone(),
        tools,
        model,
        metadata: Value::Object(rest),
    }
}

fn str_opt(entry: &Map<String, Value>, key: &str) -> Option<String> {
    entry.get(key)?.as_str().map(str::to_string)
}

fn num_opt(entry: &Map<String, Value>, key: &str) -> Option<f64> {
    entry.get(key)?.as_f64().filter(|n| n.is_finite())
}

fn hooks_from_json(raw: &Map<String, Value>) -> Vec<ConfigHook> {
    let Some(hooks) = raw.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<ConfigHook> = Vec::new();
    for (wire, entries) in hooks {
        let Some(entries) = entries.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let canonical = canonical_hook_event(wire);
            let metadata = if canonical == *wire {
                json!({ "source": "hooks.json" })
            } else {
                json!({ "source": "hooks.json", "sourceEvent": wire })
            };
            out.push(ConfigHook {
                event: canonical,
                matcher: str_opt(entry, "matcher"),
                kind: if entry.get("type").and_then(Value::as_str) == Some("prompt") {
                    HookKind::Prompt
                } else {
                    HookKind::Command
                },
                command: str_opt(entry, "command"),
                prompt: str_opt(entry, "prompt"),
                timeout_sec: num_opt(entry, "timeout"),
                fail_closed: entry.get("failClosed").and_then(Value::as_bool),
                loop_limit: num_opt(entry, "loop_limit"),
                metadata,
            });
        }
    }
    out
}

fn hook_to_cursor_entry(hook: &ConfigHook) -> Value {
    let mut entry = Map::new();
    if let Some(command) = &hook.command {
        entry.insert("command".into(), json!(command));
    }
    if hook.kind == HookKind::Prompt {
        entry.insert("type".into(), json!("prompt"));
    }
    if let Some(prompt) = &hook.prompt {
        entry.insert("prompt".into(), json!(prompt));
    }
    if let Some(matcher) = &hook.matcher {
        entry.insert("matcher".into(), json!(matcher));
    }
    if let Some(timeout) = hook.timeout_sec {
        entry.insert("timeout".into(), crate::cursor::js_number(timeout));
    }
    if let Some(fail_closed) = hook.fail_closed {
        entry.insert("failClosed".into(), json!(fail_closed));
    }
    if let Some(loop_limit) = hook.loop_limit {
        entry.insert("loop_limit".into(), crate::cursor::js_number(loop_limit));
    }
    Value::Object(entry)
}

fn merge_cursor_hooks(raw: &Map<String, Value>, hooks: &[ConfigHook]) -> Map<String, Value> {
    let existing = raw
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut merged = existing;
    for hook in hooks {
        let event = cursor_hook_event(&hook.event);
        let mut entries = merged
            .get(&event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let entry = hook_to_cursor_entry(hook);
        let entry_key = serde_json::to_string(&entry).unwrap_or_default();
        if !entries
            .iter()
            .any(|e| serde_json::to_string(e).unwrap_or_default() == entry_key)
        {
            entries.push(entry);
        }
        merged.insert(event, Value::Array(entries));
    }
    let mut out = raw.clone();
    out.insert("version".into(), json!(1));
    out.insert("hooks".into(), Value::Object(merged));
    out
}

/// Read the Cursor config under `.cursor` dir `dir` into the config IR.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn read(dir: &Path) -> Result<AgentConfig, StorageError> {
    let rules: Vec<ConfigRule> = sdk_fs::read_markdown_dir(&dir.join("rules"), &[".mdc", ".md"])?
        .iter()
        .map(doc_to_rule)
        .collect();
    let commands: Vec<ConfigCommand> = sdk_fs::read_markdown_dir(&dir.join("commands"), &[".md"])?
        .iter()
        .map(doc_to_command)
        .collect();
    let agents: Vec<ConfigAgent> = sdk_fs::read_markdown_dir(&dir.join("agents"), &[".md"])?
        .iter()
        .map(doc_to_agent)
        .collect();
    let skills = sdk_fs::read_skills_dir(&dir.join("skills"))?;

    let hooks = sdk_fs::read_json_if_exists(&dir.join("hooks.json"))?
        .map_or_else(Vec::new, |raw| hooks_from_json(&raw));

    let mcp_servers = sdk_fs::read_json_if_exists(&dir.join("mcp.json"))?
        .and_then(|raw| raw.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default();

    Ok(AgentConfig {
        skills,
        rules,
        commands,
        hooks,
        agents,
        mcp_servers,
        metadata: json!({ "source": "cursor", "dir": dir.display().to_string() }),
    })
}

fn rule_doc(rule: &ConfigRule) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &rule.description {
        attrs.insert("description".into(), json!(description));
    }
    if !rule.globs.is_empty() {
        attrs.insert("globs".into(), json!(rule.globs.join(", ")));
    }
    // `kind: "instructions"` rules land in the same rules dir, marked
    // alwaysApply — the closest Cursor gets to a memory file in-scope.
    if rule.always_apply || rule.kind == RuleKind::Instructions {
        attrs.insert("alwaysApply".into(), json!(true));
    }
    attrs.extend(extra_frontmatter(&rule.metadata));
    frontmatter::render(&attrs, &rule.body)
}

fn command_doc(command: &ConfigCommand) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &command.description {
        attrs.insert("description".into(), json!(description));
    }
    if let Some(argument_hint) = &command.argument_hint {
        attrs.insert("argument-hint".into(), json!(argument_hint));
    }
    if !command.allowed_tools.is_empty() {
        attrs.insert("allowed-tools".into(), json!(command.allowed_tools));
    }
    if let Some(model) = &command.model {
        attrs.insert("model".into(), json!(model));
    }
    attrs.extend(extra_frontmatter(&command.metadata));
    frontmatter::render(&attrs, &command.body)
}

fn agent_doc(agent: &ConfigAgent) -> String {
    let mut attrs = Map::new();
    attrs.insert("name".into(), json!(agent.name));
    if let Some(description) = &agent.description {
        attrs.insert("description".into(), json!(description));
    }
    if !agent.tools.is_empty() {
        attrs.insert("tools".into(), json!(agent.tools.join(", ")));
    }
    if let Some(model) = &agent.model {
        attrs.insert("model".into(), json!(model));
    }
    attrs.extend(extra_frontmatter(&agent.metadata));
    frontmatter::render(&attrs, &agent.body)
}

/// Write each item as `<dir>/<stem>.md`; unusable stems report `skipped`.
fn write_docs<T, F>(
    dir: &Path,
    items: &[T],
    render: F,
) -> Result<Vec<ConfigWriteAction>, StorageError>
where
    T: HasName,
    F: Fn(&T) -> String,
{
    let mut actions = Vec::new();
    for item in items {
        let Some(stem) = safe_file_stem(item.name()) else {
            actions.push(ConfigWriteAction {
                path: dir.display().to_string(),
                action: sepia_core::agent_config::WriteActionKind::Skipped,
                detail: Some(format!(
                    "{} has no usable file stem",
                    serde_json::to_string(item.name()).unwrap_or_default()
                )),
            });
            continue;
        };
        actions.push(sdk_fs::write_file_action(
            &dir.join(format!("{stem}.md")),
            &render(item),
        )?);
    }
    Ok(actions)
}

/// The name field every config doc item carries.
pub trait HasName {
    fn name(&self) -> &str;
}
impl HasName for ConfigRule {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for ConfigCommand {
    fn name(&self) -> &str {
        &self.name
    }
}
impl HasName for ConfigAgent {
    fn name(&self) -> &str {
        &self.name
    }
}

/// Write `config` into the `.cursor` dir `dir`; returns per-file actions.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn write(config: &AgentConfig, dir: &Path) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions: Vec<ConfigWriteAction> = Vec::new();

    actions.extend(write_docs(&dir.join("rules"), &config.rules, rule_doc)?);
    actions.extend(sdk_fs::write_skills_dir(
        &dir.join("skills"),
        &config.skills,
    )?);
    actions.extend(write_docs(
        &dir.join("commands"),
        &config.commands,
        command_doc,
    )?);
    actions.extend(write_docs(&dir.join("agents"), &config.agents, agent_doc)?);

    if !config.hooks.is_empty() {
        let path = dir.join("hooks.json");
        let raw = sdk_fs::read_json_if_exists(&path)?.unwrap_or_default();
        let merged = merge_cursor_hooks(&raw, &config.hooks);
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(merged)).unwrap_or_default()
        );
        actions.push(sdk_fs::write_file_action(&path, &text)?);
    }

    let mcp = merge_mcp_servers(
        sdk_fs::read_json_if_exists(&dir.join("mcp.json"))?.as_ref(),
        &config.mcp_servers,
    );
    if let Some(mcp) = mcp {
        actions.push(sdk_fs::write_file_action(
            &dir.join("mcp.json"),
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(mcp)).unwrap_or_default()
            ),
        )?);
    }

    Ok(actions)
}
