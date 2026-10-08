//! `DevinConfig.ts` port — Devin config store: `dir` is `~/.config/devin`
//! (user scope) or a repo's `.devin/` directory (project scope).
//!
//! Read:
//! - `skills/<name>/SKILL.md` + sibling files — Devin skills carry rich
//!   frontmatter (`argument-hint`, `subagent`, `allowed-tools`,
//!   `permissions`, `triggers`); leftovers ride in `metadata`.
//! - `rules/<name>.md` — plain markdown rules Devin always applies.
//! - `workflows/<name>.md` — `/name` prompt templates → `ConfigCommand`s.
//! - `AGENTS.md` inside the dir → a `kind: "instructions"` rule.
//! - `hooks.v1.json` — the Claude settings-schema hooks object
//!   (`{Event: [{matcher, hooks: [{type, command, timeout}]}]}`); hook
//!   *scripts* under `hooks/` are referenced by `command` strings and
//!   stay the user's responsibility.
//! - `config.json` (model/org preferences) is not prompt config — skipped.
//!
//! Write: skills/rules/commands symmetric; `kind: "instructions"` rules
//! merge into `AGENTS.md` under a managed `sepia:rules` block; hooks merge
//! into `hooks.v1.json`; subagent and MCP items report `skipped`.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigRule, ConfigWriteAction, RuleKind, WriteActionKind,
    hooks_from_claude_json, merge_hook_events, rules_to_memory_file, safe_file_stem,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver_sdk::fs as sdk_fs;
use serde_json::{Map, Value, json};

fn doc_to_rule(doc: &sdk_fs::MarkdownDoc) -> ConfigRule {
    let mut rest = doc.attributes.clone();
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigRule {
        name: doc.stem.clone(),
        description,
        body: doc.body.clone(),
        globs: Vec::new(),
        always_apply: true,
        kind: RuleKind::Rule,
        metadata: Value::Object(rest),
    }
}

fn doc_to_command(doc: &sdk_fs::MarkdownDoc) -> ConfigCommand {
    let mut rest = doc.attributes.clone();
    let description = rest
        .remove("description")
        .and_then(|v| v.as_str().map(str::to_string));
    rest.insert("sourcePath".into(), json!(doc.path.display().to_string()));
    ConfigCommand {
        name: doc.stem.clone(),
        description,
        body: doc.body.clone(),
        argument_hint: None,
        allowed_tools: Vec::new(),
        model: None,
        metadata: Value::Object(rest),
    }
}

/// Read the Devin config under `dir` into the config IR.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn read(dir: &Path) -> Result<AgentConfig, StorageError> {
    let skills = sdk_fs::read_skills_dir(&dir.join("skills"))?;
    let mut rules: Vec<ConfigRule> = sdk_fs::read_markdown_dir(&dir.join("rules"), &[".md"])?
        .iter()
        .map(doc_to_rule)
        .collect();
    let commands = sdk_fs::read_markdown_dir(&dir.join("workflows"), &[".md"])?
        .iter()
        .map(doc_to_command)
        .collect();

    let agents_path = dir.join("AGENTS.md");
    if let Some(agents_md) = sdk_fs::read_file_if_exists(&agents_path)? {
        rules.push(ConfigRule {
            name: "agents".into(),
            description: None,
            body: agents_md.trim_end().to_string(),
            globs: Vec::new(),
            always_apply: true,
            kind: RuleKind::Instructions,
            metadata: json!({ "sourcePath": agents_path.display().to_string() }),
        });
    }

    let hooks = sdk_fs::read_json_if_exists(&dir.join("hooks.v1.json"))?
        .map_or_else(Vec::new, |raw| {
            hooks_from_claude_json(&raw, "hooks.v1.json")
        });

    Ok(AgentConfig {
        skills,
        rules,
        commands,
        hooks,
        agents: Vec::new(),
        mcp_servers: Map::new(),
        metadata: json!({ "source": "devin", "dir": dir.display().to_string() }),
    })
}

fn rule_doc(rule: &ConfigRule) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &rule.description {
        attrs.insert("description".into(), json!(description));
    }
    frontmatter::render(&attrs, &rule.body)
}

fn command_doc(command: &ConfigCommand) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &command.description {
        attrs.insert("description".into(), json!(description));
    }
    frontmatter::render(&attrs, &command.body)
}

fn write_docs<A: HasName>(
    dir: &Path,
    items: &[A],
    render: impl Fn(&A) -> String,
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

fn skipped(path: &Path, detail: String) -> ConfigWriteAction {
    ConfigWriteAction {
        path: path.display().to_string(),
        action: WriteActionKind::Skipped,
        detail: Some(detail),
    }
}

/// Write `config` into the Devin config `dir`; returns per-file actions.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn write(config: &AgentConfig, dir: &Path) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions: Vec<ConfigWriteAction> = Vec::new();

    actions.extend(sdk_fs::write_skills_dir(
        &dir.join("skills"),
        &config.skills,
    )?);

    let file_rules: Vec<ConfigRule> = config
        .rules
        .iter()
        .filter(|r| r.kind != RuleKind::Instructions)
        .cloned()
        .collect();
    let memory_rules: Vec<ConfigRule> = config
        .rules
        .iter()
        .filter(|r| r.kind == RuleKind::Instructions)
        .cloned()
        .collect();
    actions.extend(write_docs(&dir.join("rules"), &file_rules, rule_doc)?);
    if !memory_rules.is_empty() {
        let path = dir.join("AGENTS.md");
        let existing = sdk_fs::read_file_if_exists(&path)?;
        if let Some(merged) = rules_to_memory_file(existing.as_deref(), &memory_rules) {
            actions.push(sdk_fs::write_file_action(&path, &merged)?);
        }
    }

    actions.extend(write_docs(
        &dir.join("workflows"),
        &config.commands,
        command_doc,
    )?);

    if !config.hooks.is_empty() {
        let path = dir.join("hooks.v1.json");
        let raw = sdk_fs::read_json_if_exists(&path)?.unwrap_or_default();
        let merged = merge_hook_events(&raw, &config.hooks);
        actions.push(sdk_fs::write_file_action(
            &path,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(merged)).unwrap_or_default()
            ),
        )?);
    }

    if !config.agents.is_empty() {
        actions.push(skipped(
            dir,
            format!(
                "devin has no subagent files — {} item(s) not written",
                config.agents.len()
            ),
        ));
    }
    if !config.mcp_servers.is_empty() {
        actions.push(skipped(
            dir,
            "devin has no MCP settings file — mcpServers not written".into(),
        ));
    }

    Ok(actions)
}
