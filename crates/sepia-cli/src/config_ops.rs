//! Port of `apps/sepia/src/config-commands.ts` — the `sepia config`
//! verbs over *local* agent config stores (skills, rules, commands,
//! hooks, subagents, MCP servers) via the drivers' config modules and
//! the config IR. Pure file ops: no running node, no sessions touched.
//!
//! (`sepia config get|set` — the node's server-side config — live in
//! `node_ops.rs`; clap merges both into one `config` group.)

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigRule, ConfigSkill, ConfigWriteAction,
    WriteActionKind, config_from_json, config_to_json,
};
use serde_json::Value;

use crate::CliError;

/// The four agents the config verbs read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AgentId {
    Claude,
    Cursor,
    Cline,
    Devin,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Devin => "devin",
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn default_dir(agent: AgentId) -> PathBuf {
    match agent {
        AgentId::Claude => home_dir().join(".claude"),
        AgentId::Cursor => home_dir().join(".cursor"),
        AgentId::Cline => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        AgentId::Devin => home_dir().join(".config/devin"),
    }
}

/// The four `--*-dir` flags resolved into an agent → dir map.
#[derive(Clone, Debug, Default)]
pub struct ConfigDirs {
    pub claude: Option<PathBuf>,
    pub cursor: Option<PathBuf>,
    pub cline: Option<PathBuf>,
    pub devin: Option<PathBuf>,
}

impl ConfigDirs {
    fn get(&self, agent: AgentId) -> Option<&PathBuf> {
        match agent {
            AgentId::Claude => self.claude.as_ref(),
            AgentId::Cursor => self.cursor.as_ref(),
            AgentId::Cline => self.cline.as_ref(),
            AgentId::Devin => self.devin.as_ref(),
        }
    }
    fn dir_of(&self, agent: AgentId) -> PathBuf {
        self.get(agent)
            .cloned()
            .unwrap_or_else(|| default_dir(agent))
    }
}

/// Resolve which agent a role names. An explicit `--from`/`--to` wins;
/// otherwise a single explicit `--*-dir` names its agent (the flag would
/// be noise otherwise, same convention as the session verbs); last
/// resort is `fallback`.
fn resolve_agent(
    flag: Option<AgentId>,
    dirs: &ConfigDirs,
    claimed: Option<AgentId>,
    fallback: AgentId,
) -> AgentId {
    flag.or_else(|| {
        // Declaration order — claude, cursor, cline, devin.
        [
            AgentId::Claude,
            AgentId::Cursor,
            AgentId::Cline,
            AgentId::Devin,
        ]
        .into_iter()
        .find(|a| dirs.get(*a).is_some() && Some(*a) != claimed)
    })
    .unwrap_or(fallback)
}

fn read_agent(agent: AgentId, dirs: &ConfigDirs) -> Result<AgentConfig, CliError> {
    let dir = dirs.dir_of(agent);
    match agent {
        AgentId::Claude => sepia_driver_claude::config::read(&dir),
        AgentId::Cursor => sepia_driver_cursor::config::read(&dir),
        AgentId::Cline => sepia_driver_cline::config::read(&dir),
        AgentId::Devin => sepia_driver_devin::config::read(&dir),
    }
    .map_err(|e| CliError(e.message))
}

fn write_agent(
    agent: AgentId,
    dirs: &ConfigDirs,
    config: &AgentConfig,
) -> Result<Vec<ConfigWriteAction>, CliError> {
    let dir = dirs.dir_of(agent);
    match agent {
        AgentId::Claude => sepia_driver_claude::config::write(config, &dir),
        AgentId::Cursor => sepia_driver_cursor::config::write(config, &dir),
        AgentId::Cline => sepia_driver_cline::config::write(config, &dir),
        AgentId::Devin => sepia_driver_devin::config::write(config, &dir),
    }
    .map_err(|e| CliError(e.message))
}

fn action_name(kind: WriteActionKind) -> &'static str {
    match kind {
        WriteActionKind::Wrote => "wrote",
        WriteActionKind::Updated => "updated",
        WriteActionKind::Unchanged => "unchanged",
        WriteActionKind::Merged => "merged",
        WriteActionKind::Skipped => "skipped",
    }
}

fn print_actions(actions: &[ConfigWriteAction], target: AgentId) {
    for a in actions {
        println!(
            "{:9} {}{}",
            action_name(a.action),
            a.path,
            a.detail
                .as_ref()
                .map_or(String::new(), |d| format!(" — {d}"))
        );
    }
    let written = actions
        .iter()
        .filter(|a| {
            matches!(
                a.action,
                WriteActionKind::Wrote | WriteActionKind::Updated | WriteActionKind::Merged
            )
        })
        .count();
    println!(
        "{written} file(s) written into the {} config",
        target.as_str()
    );
}

trait Named {
    fn name(&self) -> &str;
}
impl Named for ConfigRule {
    fn name(&self) -> &str {
        &self.name
    }
}
impl Named for ConfigCommand {
    fn name(&self) -> &str {
        &self.name
    }
}
impl Named for ConfigSkill {
    fn name(&self) -> &str {
        &self.name
    }
}
impl Named for ConfigAgent {
    fn name(&self) -> &str {
        &self.name
    }
}

fn summarize(config: &AgentConfig) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let section = |lines: &mut Vec<String>, title: &str, items: &[&dyn Named]| {
        if items.is_empty() {
            return;
        }
        lines.push(format!("{title} ({}):", items.len()));
        for item in items {
            lines.push(format!("  {}", item.name()));
        }
    };
    section(
        &mut lines,
        "Rules",
        &config
            .rules
            .iter()
            .map(|r| r as &dyn Named)
            .collect::<Vec<_>>(),
    );
    section(
        &mut lines,
        "Commands",
        &config
            .commands
            .iter()
            .map(|c| c as &dyn Named)
            .collect::<Vec<_>>(),
    );
    section(
        &mut lines,
        "Skills",
        &config
            .skills
            .iter()
            .map(|s| s as &dyn Named)
            .collect::<Vec<_>>(),
    );
    section(
        &mut lines,
        "Subagents",
        &config
            .agents
            .iter()
            .map(|a| a as &dyn Named)
            .collect::<Vec<_>>(),
    );
    if !config.hooks.is_empty() {
        lines.push(format!("Hooks ({}):", config.hooks.len()));
        for hook in &config.hooks {
            let cmd = match (&hook.command, &hook.prompt) {
                (Some(c), _) => c.clone(),
                (None, Some(_)) => "[prompt]".to_string(),
                (None, None) => String::new(),
            };
            lines.push(format!(
                "  {}{} → {}",
                hook.event,
                hook.matcher
                    .as_ref()
                    .map_or(String::new(), |m| format!(" ~ {m}")),
                cmd
            ));
        }
    }
    if !config.mcp_servers.is_empty() {
        let names: Vec<&str> = config.mcp_servers.keys().map(String::as_str).collect();
        lines.push(format!(
            "MCP servers ({}): {}",
            names.len(),
            names.join(", ")
        ));
    }
    if lines.is_empty() {
        vec!["(empty config)".to_string()]
    } else {
        lines
    }
}

/// `sepia config list` — summarize an agent's config items.
pub fn config_list(from: Option<AgentId>, dirs: &ConfigDirs) -> Result<(), CliError> {
    let source = resolve_agent(from, dirs, None, AgentId::Claude);
    let config = read_agent(source, dirs)?;
    println!(
        "# {} config — {}",
        source.as_str(),
        dirs.dir_of(source).display()
    );
    for line in summarize(&config) {
        println!("{line}");
    }
    Ok(())
}

/// `sepia config export [out]` — the config IR JSON to stdout or a file.
pub fn config_export(
    out: Option<&Path>,
    from: Option<AgentId>,
    dirs: &ConfigDirs,
) -> Result<(), CliError> {
    let source = resolve_agent(from, dirs, None, AgentId::Claude);
    let config = read_agent(source, dirs)?;
    let json = serde_json::to_string_pretty(&config_to_json(&config))
        .map_err(|e| CliError(format!("Failed to encode config: {e}")))?;
    match out {
        None => println!("{json}"),
        Some(o) if o.as_os_str() == "-" => println!("{json}"),
        Some(o) => {
            std::fs::write(o, format!("{json}\n"))
                .map_err(|e| CliError(format!("Failed to write {}: {e}", o.display())))?;
            println!("Exported {} config to {}", source.as_str(), o.display());
        }
    }
    Ok(())
}

/// `sepia config import <path>` — install a config IR JSON file into an
/// agent's config store.
pub fn config_import(path: &Path, to: Option<AgentId>, dirs: &ConfigDirs) -> Result<(), CliError> {
    let target = resolve_agent(to, dirs, None, AgentId::Cursor);
    let raw = std::fs::read_to_string(path)
        .map_err(|_| CliError(format!("Config source not found: {}", path.display())))?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|_| {
        CliError(format!(
            "Config source is not a config IR JSON: {}",
            path.display()
        ))
    })?;
    let config = config_from_json(&parsed).map_err(|_| {
        CliError(format!(
            "Config source is not a config IR JSON: {}",
            path.display()
        ))
    })?;
    let actions = write_agent(target, dirs, &config)?;
    print_actions(&actions, target);
    Ok(())
}

/// `sepia config install` — copy one agent's config into another's
/// store (IR → target's files).
pub fn config_install(
    from: Option<AgentId>,
    to: Option<AgentId>,
    dirs: &ConfigDirs,
) -> Result<(), CliError> {
    let target = resolve_agent(to, dirs, None, AgentId::Cursor);
    let source = resolve_agent(from, dirs, Some(target), AgentId::Claude);
    let config = read_agent(source, dirs)?;
    let actions = write_agent(target, dirs, &config)?;
    println!(
        "Installed {} config into {} ({})",
        source.as_str(),
        target.as_str(),
        dirs.dir_of(target).display()
    );
    print_actions(&actions, target);
    Ok(())
}

/// Strip per-source provenance so a diff compares content, not origin.
fn norm(item: &Value) -> String {
    let mut clone = item.clone();
    if let Value::Object(map) = &mut clone {
        map.remove("metadata");
    }
    serde_json::to_string(&clone).unwrap_or_default()
}

fn name_of(v: &Value) -> &str {
    v.get("name").and_then(Value::as_str).unwrap_or_default()
}

fn diff_kind(label: &str, a: &[Value], b: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for item in a {
        let name = name_of(item);
        match b.iter().find(|o| name_of(o) == name) {
            None => lines.push(format!("  {label} only in source: {name}")),
            Some(other) if norm(item) != norm(other) => {
                lines.push(format!("  {label} changed: {name}"));
            }
            _ => {}
        }
    }
    for item in b {
        let name = name_of(item);
        if !a.iter().any(|o| name_of(o) == name) {
            lines.push(format!("  {label} only in target: {name}"));
        }
    }
    lines
}

/// `sepia config diff` — compare two agents' configs (name-level, plus
/// hook/MCP deltas).
pub fn config_diff(
    from: Option<AgentId>,
    to: Option<AgentId>,
    dirs: &ConfigDirs,
) -> Result<(), CliError> {
    let target = resolve_agent(to, dirs, None, AgentId::Cursor);
    let source = resolve_agent(from, dirs, Some(target), AgentId::Claude);
    let a = read_agent(source, dirs)?;
    let b = read_agent(target, dirs)?;
    let a_v = serde_json::to_value(&a).unwrap_or_default();
    let b_v = serde_json::to_value(&b).unwrap_or_default();
    let list = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut lines = [
        diff_kind("rule", &list(&a_v, "rules"), &list(&b_v, "rules")),
        diff_kind("command", &list(&a_v, "commands"), &list(&b_v, "commands")),
        diff_kind("skill", &list(&a_v, "skills"), &list(&b_v, "skills")),
        diff_kind("subagent", &list(&a_v, "agents"), &list(&b_v, "agents")),
    ]
    .concat();

    let hook_key = |h: &sepia_core::agent_config::ConfigHook| {
        format!(
            "{}:{}",
            h.event,
            serde_json::to_string(h).unwrap_or_default()
        )
    };
    let a_hooks: std::collections::HashSet<String> = a.hooks.iter().map(hook_key).collect();
    let b_hooks: std::collections::HashSet<String> = b.hooks.iter().map(hook_key).collect();
    let only_a = a
        .hooks
        .iter()
        .filter(|h| !b_hooks.contains(&hook_key(h)))
        .count();
    let only_b = b
        .hooks
        .iter()
        .filter(|h| !a_hooks.contains(&hook_key(h)))
        .count();
    if only_a > 0 {
        lines.push(format!("  hooks only in source: {only_a}"));
    }
    if only_b > 0 {
        lines.push(format!("  hooks only in target: {only_b}"));
    }
    for (name, server) in &a.mcp_servers {
        match b.mcp_servers.get(name) {
            None => lines.push(format!("  mcp server only in source: {name}")),
            Some(other)
                if serde_json::to_string(server).unwrap_or_default()
                    != serde_json::to_string(other).unwrap_or_default() =>
            {
                lines.push(format!("  mcp server changed: {name}"));
            }
            _ => {}
        }
    }
    for name in b.mcp_servers.keys() {
        if !a.mcp_servers.contains_key(name) {
            lines.push(format!("  mcp server only in target: {name}"));
        }
    }
    println!("# {} → {}", source.as_str(), target.as_str());
    if lines.is_empty() {
        println!("  configs equivalent");
    } else {
        for line in &lines {
            println!("{line}");
        }
    }
    Ok(())
}
