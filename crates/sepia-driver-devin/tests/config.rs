#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Config-store tests: `.devin` dir read/write — skills, `rules/`,
//! `workflows/`, `AGENTS.md` memory, `hooks.v1.json`, and the honestly
//! `skipped` surfaces (subagents, MCP).

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigHook, ConfigRule, HookKind, RuleKind,
    WriteActionKind,
};
use sepia_driver_devin::config;
use serde_json::{Value, json};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn rule(name: &str, kind: RuleKind, body: &str) -> ConfigRule {
    ConfigRule {
        name: name.into(),
        description: None,
        body: body.into(),
        globs: Vec::new(),
        always_apply: true,
        kind,
        metadata: Value::Null,
    }
}

#[test]
fn read_write_round_trip_is_stable_and_lossless() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("AGENTS.md"), "Dev rules.\n");
    write(
        &dir.path().join("rules/style.md"),
        "---\ndescription: Style\nteam: platform\n---\n\nNo comments.\n",
    );
    write(
        &dir.path().join("workflows/review.md"),
        "---\ndescription: Review\ntier: slow\n---\n\nReview the diff.\n",
    );
    write(
        &dir.path().join("hooks.v1.json"),
        r#"{"Stop":[{"hooks":[{"type":"command","command":"done.sh"}]}]}"#,
    );
    write(
        &dir.path().join("skills/ship/SKILL.md"),
        "---\nname: ship\ndescription: Ship it\nsubagent: release\n---\n\nShip.\n",
    );
    write(&dir.path().join("skills/ship/run.sh"), "#!/bin/sh\n");

    let config = config::read(dir.path()).unwrap();
    let actions = config::write(&config, dir.path()).unwrap();

    // AGENTS.md is owned by its instructions rule — verbatim, not a
    // `## agents` block section.
    let agents_md = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(agents_md, "Dev rules.\n");
    let memory_action = actions
        .iter()
        .find(|a| a.path.ends_with("AGENTS.md"))
        .unwrap();
    assert_eq!(memory_action.action, WriteActionKind::Unchanged);

    // Leftover frontmatter (`team`, `tier`, `subagent`) round-trips.
    let style = std::fs::read_to_string(dir.path().join("rules/style.md")).unwrap();
    assert!(style.contains("team: platform"));
    let review = std::fs::read_to_string(dir.path().join("workflows/review.md")).unwrap();
    assert!(review.contains("tier: slow"));
    let skill = std::fs::read_to_string(dir.path().join("skills/ship/SKILL.md")).unwrap();
    assert!(skill.contains("subagent: release"));

    // Re-read reproduces the same IR.
    let back = config::read(dir.path()).unwrap();
    assert_eq!(back.rules.len(), 2);
    assert_eq!(back.rules[1].kind, RuleKind::Instructions);
    assert_eq!(back.rules[1].body, "Dev rules.");
    assert_eq!(back.hooks.len(), 1);
    assert_eq!(back.commands[0].body, "Review the diff.\n");
}

#[test]
fn write_reports_skipped_and_dropped_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut scoped = rule("scoped", RuleKind::Rule, "body");
    scoped.globs = vec!["*.rs".into()];
    scoped.always_apply = false;
    let hook = ConfigHook {
        event: "Stop".into(),
        matcher: None,
        kind: HookKind::Command,
        command: Some("x.sh".into()),
        prompt: None,
        timeout_sec: None,
        fail_closed: Some(true),
        loop_limit: None,
        metadata: Value::Null,
    };
    let config = AgentConfig {
        rules: vec![scoped],
        commands: vec![ConfigCommand {
            name: "go".into(),
            description: None,
            body: "run".into(),
            argument_hint: Some("[x]".into()),
            allowed_tools: vec!["Bash".into()],
            model: Some("opus".into()),
            metadata: Value::Null,
        }],
        hooks: vec![hook],
        agents: vec![ConfigAgent {
            name: "a".into(),
            description: None,
            body: String::new(),
            tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        mcp_servers: serde_json::Map::from_iter([("s".into(), json!({}))]),
        ..AgentConfig::default()
    };
    let actions = config::write(&config, dir.path()).unwrap();

    let rule_action = actions
        .iter()
        .find(|a| a.path.ends_with("rules/scoped.md"))
        .unwrap();
    let detail = rule_action.detail.as_deref().unwrap_or("");
    assert!(detail.contains("globs") && detail.contains("alwaysApply"));

    let cmd_action = actions
        .iter()
        .find(|a| a.path.ends_with("workflows/go.md"))
        .unwrap();
    let detail = cmd_action.detail.as_deref().unwrap_or("");
    assert!(detail.contains("argumentHint") && detail.contains("model"));

    let hook_action = actions
        .iter()
        .find(|a| a.path.ends_with("hooks.v1.json"))
        .unwrap();
    assert!(
        hook_action
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("failClosed")
    );

    let skipped: Vec<_> = actions
        .iter()
        .filter(|a| a.action == WriteActionKind::Skipped)
        .collect();
    assert_eq!(skipped.len(), 2); // subagents + mcpServers
    assert!(
        skipped
            .iter()
            .any(|a| a.detail.as_deref().unwrap_or("").contains("subagent"))
    );
    assert!(
        skipped
            .iter()
            .any(|a| a.detail.as_deref().unwrap_or("").contains("MCP"))
    );
}

#[test]
fn foreign_instructions_rules_merge_into_agents_md_block() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("AGENTS.md"), "Keep this.\n");
    let config = AgentConfig {
        rules: vec![
            rule("claude", RuleKind::Instructions, "Claude memory."),
            rule("extra", RuleKind::Rule, "Extra rule."),
        ],
        ..AgentConfig::default()
    };
    config::write(&config, dir.path()).unwrap();
    let agents_md = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("Keep this."));
    assert!(agents_md.contains("<!-- sepia:rules -->"));
    assert!(agents_md.contains("## claude"));
    // Plain rules still land as files.
    assert!(
        std::fs::read_to_string(dir.path().join("rules/extra.md"))
            .unwrap()
            .contains("Extra rule.")
    );
}
