//! Cline workspace config: `dir` is the workspace
//! root (where a session runs), not `~/.cline/data`: Cline's portable
//! config lives next to the code.
//!
//! Read:
//! - `.clinerules` — a plain-text file → one `kind: "instructions"` rule.
//! - `.clinerules/` — a directory of `*.md` files → rules (Cline applies
//!   every rule file unconditionally, so `always_apply: true`). A nested
//!   `workflows/` dir holds slash-command markdown → `ConfigCommand`s.
//! - `cline_mcp_settings.json` `mcpServers`.
//!
//! Write: rules land as `.clinerules/<name>.md` files (the directory form —
//! always valid, unlike the single `.clinerules` file), commands as
//! `.clinerules/workflows/<name>.md`, MCP merged into
//! `cline_mcp_settings.json`. When `.clinerules` already exists *as a file*,
//! the instructions rule read from it owns the file and writes back
//! verbatim; other rules merge into a managed `sepia:rules` block there,
//! and commands report `skipped` (the workflows dir can't exist under a
//! file). Cline has no skills/subagents/hooks concept — those items are
//! reported `skipped`.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigRule, ConfigWriteAction, RuleKind, WriteActionKind,
    extra_frontmatter, merge_mcp_servers, merge_memory_file, safe_file_stem, source_path,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver::fs as sdk_fs;
use serde_json::{Map, Value, json};

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

/// Read the Cline workspace config under `dir` into the config IR.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn read(dir: &Path) -> Result<AgentConfig, StorageError> {
    let mut rules: Vec<ConfigRule> = Vec::new();
    let rules_path = dir.join(".clinerules");
    match std::fs::metadata(&rules_path) {
        Ok(info) if info.is_file() => {
            if let Some(text) = sdk_fs::read_file_if_exists(&rules_path)? {
                rules.push(ConfigRule {
                    name: "clinerules".into(),
                    description: None,
                    body: text.trim_end().to_string(),
                    globs: Vec::new(),
                    always_apply: true,
                    kind: RuleKind::Instructions,
                    metadata: json!({ "sourcePath": rules_path.display().to_string() }),
                });
            }
        }
        Ok(info) if info.is_dir() => {
            rules.extend(
                sdk_fs::read_markdown_dir(&rules_path, &[".md"])?
                    .iter()
                    .map(doc_to_rule),
            );
        }
        _ => {}
    }

    let commands = sdk_fs::read_markdown_dir(&rules_path.join("workflows"), &[".md"])?
        .iter()
        .map(doc_to_command)
        .collect();

    let mcp_json = sdk_fs::read_json_if_exists(&dir.join("cline_mcp_settings.json"))?;
    let mcp_servers = mcp_json
        .and_then(|m| m.get("mcpServers").and_then(Value::as_object).cloned())
        .unwrap_or_default();

    Ok(AgentConfig {
        skills: Vec::new(),
        rules,
        commands,
        hooks: Vec::new(),
        agents: Vec::new(),
        mcp_servers,
        metadata: json!({ "source": "cline", "dir": dir.display().to_string() }),
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

/// Scoping fields `.clinerules` can't express — every rule file applies to
/// every task, so `globs`/`alwaysApply: false` are dropped (and noted).
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
            "{} not written — .clinerules rules apply unconditionally",
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

/// Command fields Cline's workflows don't read — the files are plain prompt
/// markdown, so `argumentHint`/`allowedTools`/`model` are dropped (and noted).
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
            "{} not written — workflows are plain prompts",
            dropped.join("/")
        )
    })
}

fn write_docs<A>(
    dir: &Path,
    items: &[A],
    render: impl Fn(&A) -> String,
    name_of: impl Fn(&A) -> &str,
    note: impl Fn(&A) -> Option<String>,
) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions = Vec::new();
    for item in items {
        let Some(stem) = safe_file_stem(name_of(item)) else {
            actions.push(ConfigWriteAction {
                path: dir.display().to_string(),
                action: WriteActionKind::Skipped,
                detail: Some(format!(
                    "{} has no usable file stem",
                    serde_json::to_string(name_of(item)).unwrap_or_default()
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

fn skipped(kind: &str, count: usize) -> Option<ConfigWriteAction> {
    if count == 0 {
        return None;
    }
    Some(ConfigWriteAction {
        path: ".clinerules".into(),
        action: WriteActionKind::Skipped,
        detail: Some(format!(
            "cline has no {kind} concept — {count} item(s) not written"
        )),
    })
}

/// Write `config` into the Cline workspace `dir`; returns per-file actions.
///
/// # Errors
/// `StorageError` on filesystem failures.
pub fn write(config: &AgentConfig, dir: &Path) -> Result<Vec<ConfigWriteAction>, StorageError> {
    let mut actions: Vec<ConfigWriteAction> = Vec::new();

    let rules_path = dir.join(".clinerules");
    if std::fs::metadata(&rules_path).is_ok_and(|m| m.is_file()) {
        // File form: the instructions rule read from it owns the file;
        // other rules merge into its managed block. Commands need the
        // directory form — honestly skipped.
        let source = rules_path.display().to_string();
        let mut owners: Vec<ConfigRule> = Vec::new();
        let mut block_rules: Vec<ConfigRule> = Vec::new();
        for rule in &config.rules {
            if rule.kind == RuleKind::Instructions
                && source_path(&rule.metadata) == Some(source.as_str())
            {
                owners.push(rule.clone());
            } else {
                block_rules.push(rule.clone());
            }
        }
        if !owners.is_empty() || !block_rules.is_empty() {
            let existing = sdk_fs::read_file_if_exists(&rules_path)?;
            // Owners or blocks are non-empty past the guard, so the
            // merge always produces content.
            actions.extend(
                merge_memory_file(existing.as_deref(), &owners, &block_rules)
                    .map(|merged| sdk_fs::write_file_action(&rules_path, &merged))
                    .transpose()?,
            );
        }
        if !config.commands.is_empty() {
            actions.push(ConfigWriteAction {
                path: rules_path.join("workflows").display().to_string(),
                action: WriteActionKind::Skipped,
                detail: Some(format!(
                    ".clinerules is a file — {} command(s) need the directory form",
                    config.commands.len()
                )),
            });
        }
    } else {
        actions.extend(write_docs(
            &rules_path,
            &config.rules,
            rule_doc,
            |r| &r.name,
            rule_note,
        )?);
        actions.extend(write_docs(
            &rules_path.join("workflows"),
            &config.commands,
            command_doc,
            |c| &c.name,
            command_note,
        )?);
    }

    let mcp_path = dir.join("cline_mcp_settings.json");
    let mcp = merge_mcp_servers(
        sdk_fs::read_json_if_exists(&mcp_path)?.as_ref(),
        &config.mcp_servers,
    );
    if let Some(mcp) = mcp {
        actions.push(sdk_fs::write_file_action(
            &mcp_path,
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&mcp).unwrap_or_default()
            ),
        )?);
    }

    for action in [
        skipped("skills", config.skills.len()),
        skipped("subagents", config.agents.len()),
        skipped("hooks", config.hooks.len()),
    ]
    .into_iter()
    .flatten()
    {
        actions.push(action);
    }

    Ok(actions)
}
