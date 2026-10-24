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
//! Write: skills/rules/commands symmetric; the instructions rule read from
//! `AGENTS.md` owns that file and writes back verbatim, while other
//! `kind: "instructions"` rules merge under a managed `sepia:rules` block;
//! hooks merge into `hooks.v1.json`; subagent and MCP items report
//! `skipped`.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigRule, ConfigWriteAction, RuleKind, WriteActionKind,
    dropped_hook_fields, extra_frontmatter, hooks_from_claude_json, merge_hook_events,
    merge_memory_file, safe_file_stem, source_path,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver::fs as sdk_fs;
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
    attrs.extend(extra_frontmatter(&rule.metadata));
    frontmatter::render(&attrs, &rule.body)
}

/// Scoping fields Devin's rules can't express — `rules/*.md` files apply
/// unconditionally, so `globs`/`alwaysApply: false` are dropped (and noted).
fn rule_note(rule: &ConfigRule) -> Option<String> {
    let mut dropped = Vec::new();
    if !rule.globs.is_empty() {
        dropped.push("globs");
    }
    if !rule.always_apply {
        dropped.push("alwaysApply");
    }
    (!dropped.is_empty()).then(|| {
        format!(
            "{} not written — devin rules apply unconditionally",
            dropped.join("/")
        )
    })
}

fn command_doc(command: &ConfigCommand) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &command.description {
        attrs.insert("description".into(), json!(description));
    }
    attrs.extend(extra_frontmatter(&command.metadata));
    frontmatter::render(&attrs, &command.body)
}

/// Command fields Devin workflows don't read — only `description`
/// frontmatter is documented, so the rest is dropped (and noted).
fn command_note(command: &ConfigCommand) -> Option<String> {
    let mut dropped = Vec::new();
    if command.argument_hint.is_some() {
        dropped.push("argumentHint");
    }
    if !command.allowed_tools.is_empty() {
        dropped.push("allowedTools");
    }
    if command.model.is_some() {
        dropped.push("model");
    }
    (!dropped.is_empty()).then(|| {
        format!(
            "{} not written — workflows carry description only",
            dropped.join("/")
        )
    })
}

fn write_docs<A: HasName>(
    dir: &Path,
    items: &[A],
    render: impl Fn(&A) -> String,
    note: impl Fn(&A) -> Option<String>,
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
        let mut action = sdk_fs::write_file_action(&dir.join(format!("{stem}.md")), &render(item))?;
        action.detail = note(item);
        actions.push(action);
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

    // `rules/*.md` gets plain rules; `AGENTS.md` is owned by the
    // instructions rule read from it — other instructions rules merge into
    // its managed `sepia:rules` block.
    let agents_path = dir.join("AGENTS.md");
    let agents_source = agents_path.display().to_string();
    let mut file_rules: Vec<ConfigRule> = Vec::new();
    let mut owners: Vec<ConfigRule> = Vec::new();
    let mut block_rules: Vec<ConfigRule> = Vec::new();
    for rule in &config.rules {
        if rule.kind != RuleKind::Instructions {
            file_rules.push(rule.clone());
        } else if source_path(&rule.metadata) == Some(agents_source.as_str()) {
            owners.push(rule.clone());
        } else {
            block_rules.push(rule.clone());
        }
    }
    actions.extend(write_docs(
        &dir.join("rules"),
        &file_rules,
        rule_doc,
        rule_note,
    )?);
    if !owners.is_empty() || !block_rules.is_empty() {
        let existing = sdk_fs::read_file_if_exists(&agents_path)?;
        // Owners or blocks are non-empty past the guard, so the merge
        // always produces content.
        actions.extend(
            merge_memory_file(existing.as_deref(), &owners, &block_rules)
                .map(|merged| sdk_fs::write_file_action(&agents_path, &merged))
                .transpose()?,
        );
    }

    actions.extend(write_docs(
        &dir.join("workflows"),
        &config.commands,
        command_doc,
        command_note,
    )?);

    if !config.hooks.is_empty() {
        let path = dir.join("hooks.v1.json");
        let raw = sdk_fs::read_json_if_exists(&path)?.unwrap_or_default();
        let merged = merge_hook_events(&raw, &config.hooks);
        let mut action = sdk_fs::write_file_action(
            &path,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(merged)).unwrap_or_default()
            ),
        )?;
        action.detail = dropped_hook_fields(&config.hooks);
        actions.push(action);
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
