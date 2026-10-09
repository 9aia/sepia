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
//! `cline_mcp_settings.json`. Cline has no skills/subagents/hooks concept —
//! those items are reported `skipped`.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigRule, ConfigWriteAction, RuleKind, WriteActionKind,
    merge_mcp_servers, safe_file_stem,
};
use sepia_core::domain::StorageError;
use sepia_core::frontmatter;
use sepia_driver_sdk::fs as sdk_fs;
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
    frontmatter::render(&attrs, &rule.body)
}

fn command_doc(command: &ConfigCommand) -> String {
    let mut attrs = Map::new();
    if let Some(description) = &command.description {
        attrs.insert("description".into(), json!(description));
    }
    frontmatter::render(&attrs, &command.body)
}

fn write_docs<A>(
    dir: &Path,
    items: &[A],
    render: impl Fn(&A) -> String,
    name_of: impl Fn(&A) -> &str,
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
        actions.push(sdk_fs::write_file_action(
            &dir.join(format!("{stem}.md")),
            &render(item),
        )?);
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

    actions.extend(write_docs(
        &dir.join(".clinerules"),
        &config.rules,
        rule_doc,
        |r| &r.name,
    )?);
    actions.extend(write_docs(
        &dir.join(".clinerules").join("workflows"),
        &config.commands,
        command_doc,
        |c| &c.name,
    )?);

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
