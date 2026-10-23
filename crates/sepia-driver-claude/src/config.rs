//! Claude Code config store: `dir` is a
//! `.claude` directory, `~/.claude` (user scope) or `<repo>/.claude`
//! (project scope).
//!
//! Read:
//! - `CLAUDE.md` inside the dir (memory → `kind: "instructions"`), plus
//!   the parent `CLAUDE.md` for a project `.claude` dir
//!   (`claude-project`).
//! - `skills/<name>/SKILL.md` + sibling files, `commands/<name>.md`,
//!   `agents/<name>.md`.
//! - `hooks` from `settings.json` and `settings.local.json` (each hook's
//!   `metadata.source` records which).
//! - `mcpServers` from `~/.claude.json` (user dir) or the sibling
//!   `.mcp.json` (project dir).
//!
//! Write: skills/commands/agents map one-to-one; hooks merge into
//! `settings.json`/`settings.local.json` preserving other keys (each hook's
//! `metadata.source` picks the file it came from); an instructions rule
//! whose `sourcePath` is a managed memory file writes back as that file's
//! content, and every other rule collapses into the managed `sepia:rules`
//! block inside `CLAUDE.md` (Claude has no rules directory); `mcpServers`
//! merge into `~/.claude.json` or the sibling `.mcp.json`.

use std::path::{Path, PathBuf};

use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigHook, ConfigRule, ConfigWriteAction, RuleKind,
    WriteActionKind, dropped_hook_fields, extra_frontmatter, hooks_from_claude_json,
    merge_claude_hooks, merge_mcp_servers, merge_memory_file, safe_file_stem, source_path,
    string_list,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver::fs as sdk_fs;
use serde_json::{Map, Value, json};

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn is_user_dir(dir: &Path) -> bool {
    dir == home_dir().join(".claude")
}

fn is_project_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == ".claude") && !is_user_dir(dir)
}

fn project_root(dir: &Path) -> PathBuf {
    dir.parent()
        .map_or_else(|| dir.to_path_buf(), Path::to_path_buf)
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
        .map(|v| string_list(&v))
        .unwrap_or_default();
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
        .and_then(|v| v.as_str().map(str::to_string));
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    let tools = rest
        .remove("tools")
        .map(|v| string_list(&v))
        .unwrap_or_default();
    let model = rest
        .remove("model")
        .and_then(|v| v.as_str().map(str::to_string));
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigAgent {
        name: name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| doc.stem.clone()),
        description,
        body: doc.body.clone(),
        tools,
        model,
        metadata: Value::Object(rest),
    }
}

fn instructions_rule(name: &str, path: &Path, body: &str) -> ConfigRule {
    ConfigRule {
        name: name.into(),
        description: None,
        body: body.trim_end().to_string(),
        globs: Vec::new(),
        always_apply: true,
        kind: RuleKind::Instructions,
        metadata: json!({ "sourcePath": path.display().to_string() }),
    }
}

/// Where MCP servers live for `dir`: `~/.claude.json` for the user dir,
/// the sibling `.mcp.json` for a project dir, `<dir>/.mcp.json` else.
fn mcp_path(dir: &Path) -> PathBuf {
    if is_user_dir(dir) {
        home_dir().join(".claude.json")
    } else if is_project_dir(dir) {
        project_root(dir).join(".mcp.json")
    } else {
        dir.join(".mcp.json")
    }
}

/// Read the Claude Code config under `.claude` dir `dir` into the config IR.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn read(dir: &Path) -> Result<AgentConfig, StorageError> {
    let mut rules: Vec<ConfigRule> = Vec::new();
    let memory_path = dir.join("CLAUDE.md");
    if let Some(memory) = sdk_fs::read_file_if_exists(&memory_path)? {
        rules.push(instructions_rule("claude", &memory_path, &memory));
    }
    if is_project_dir(dir) {
        let project_memory_path = project_root(dir).join("CLAUDE.md");
        if let Some(project_memory) = sdk_fs::read_file_if_exists(&project_memory_path)? {
            rules.push(instructions_rule(
                "claude-project",
                &project_memory_path,
                &project_memory,
            ));
        }
    }

    let skills = sdk_fs::read_skills_dir(&dir.join("skills"))?;
    let commands = sdk_fs::read_markdown_dir(&dir.join("commands"), &[".md"])?
        .iter()
        .map(doc_to_command)
        .collect();
    let agents = sdk_fs::read_markdown_dir(&dir.join("agents"), &[".md"])?
        .iter()
        .map(doc_to_agent)
        .collect();

    let mut hooks = sdk_fs::read_json_if_exists(&dir.join("settings.json"))?
        .map_or_else(Vec::new, |s| hooks_from_claude_json(&s, "settings.json"));
    hooks.extend(
        sdk_fs::read_json_if_exists(&dir.join("settings.local.json"))?.map_or_else(Vec::new, |s| {
            hooks_from_claude_json(&s, "settings.local.json")
        }),
    );

    let mcp_file = sdk_fs::read_json_if_exists(&mcp_path(dir))?;
    let mcp_servers = mcp_file
        .and_then(|m| m.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default();

    Ok(AgentConfig {
        skills,
        rules,
        commands,
        hooks,
        agents,
        mcp_servers,
        metadata: json!({ "source": "claude", "dir": dir.display().to_string() }),
    })
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

trait HasName {
    fn name(&self) -> &str;
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

/// Write each item as `<dir>/<stem>.md`; unusable stems report `skipped`.
fn write_docs<T: HasName>(
    dir: &Path,
    items: &[T],
    render: impl Fn(&T) -> String,
) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions = Vec::new();
    for item in items {
        let Some(stem) = safe_file_stem(item.name()) else {
            actions.push(ConfigWriteAction {
                path: dir.display().to_string(),
                action: WriteActionKind::Skipped,
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

/// Write `config` into the `.claude` dir `dir`; returns per-file actions.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn write(config: &AgentConfig, dir: &Path) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions: Vec<ConfigWriteAction> = Vec::new();

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

    // Memory files this dir manages: `<dir>/CLAUDE.md`, plus the
    // project-root `CLAUDE.md` for a project `.claude` dir. An instructions
    // rule sourced from one of them owns the file; every other rule
    // collapses into the managed block of `<dir>/CLAUDE.md`.
    let mut memories = vec![(dir.join("CLAUDE.md"), Vec::new())];
    if is_project_dir(dir) {
        memories.push((project_root(dir).join("CLAUDE.md"), Vec::new()));
    }
    let mut block_rules: Vec<ConfigRule> = Vec::new();
    for rule in &config.rules {
        let target = if rule.kind == RuleKind::Instructions {
            source_path(&rule.metadata).and_then(|source| {
                memories
                    .iter()
                    .position(|(path, _)| path.display().to_string() == source)
            })
        } else {
            None
        };
        match target {
            Some(i) => memories[i].1.push(rule.clone()),
            None => block_rules.push(rule.clone()),
        }
    }
    for (i, (path, owners)) in memories.iter().enumerate() {
        let blocks: &[ConfigRule] = if i == 0 { &block_rules } else { &[] };
        if owners.is_empty() && blocks.is_empty() {
            continue;
        }
        let existing = sdk_fs::read_file_if_exists(path)?;
        if let Some(merged) = merge_memory_file(existing.as_deref(), owners, blocks) {
            actions.push(sdk_fs::write_file_action(path, &merged)?);
        }
    }

    // `metadata.source` routes each hook back to the file it was read from;
    // foreign hooks land in `settings.json`.
    let mut settings_hooks: Vec<ConfigHook> = Vec::new();
    let mut local_hooks: Vec<ConfigHook> = Vec::new();
    for hook in &config.hooks {
        if hook.metadata.get("source").and_then(Value::as_str) == Some("settings.local.json") {
            local_hooks.push(hook.clone());
        } else {
            settings_hooks.push(hook.clone());
        }
    }
    for (file, hooks) in [
        ("settings.json", settings_hooks),
        ("settings.local.json", local_hooks),
    ] {
        if hooks.is_empty() {
            continue;
        }
        let path = dir.join(file);
        let settings = sdk_fs::read_json_if_exists(&path)?.unwrap_or_default();
        let merged = merge_claude_hooks(&settings, &hooks);
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(merged)).unwrap_or_default()
        );
        // `write_file_action` already resolves an identical file to
        // `unchanged`.
        let mut action = sdk_fs::write_file_action(&path, &text)?;
        action.detail = dropped_hook_fields(&hooks);
        actions.push(action);
    }

    let mcp_path = mcp_path(dir);
    let mcp = merge_mcp_servers(
        sdk_fs::read_json_if_exists(&mcp_path)?.as_ref(),
        &config.mcp_servers,
    );
    if let Some(mcp) = mcp {
        actions.push(sdk_fs::write_file_action(
            &mcp_path,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(mcp)).unwrap_or_default()
            ),
        )?);
    }

    Ok(actions)
}
