#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `CursorConfig.test.ts` ports — `rules/*.md|c`, `commands/`, `agents/`,
//! `skills/`, `hooks.json` and `mcp.json` under a `.cursor` dir.

use std::path::Path;

use sepia_core::agent_config::{AgentConfig, ConfigHook, ConfigRule, HookKind, WriteActionKind};
use sepia_driver_cursor::config;
use serde_json::{Value, json};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn reads_cursor_layout_rules_commands_agents_skills_hooks_and_mcp() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("rules/rust.mdc"),
        "---\ndescription: Rust rules\nglobs: \"*.rs, *.toml\"\nalwaysApply: true\n---\n\nUse cargo.\n",
    );
    write(
        &dir.path().join("commands/review.md"),
        "---\ndescription: Review PR\n---\n\nReview the diff.\n",
    );
    write(
        &dir.path().join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews PRs\ntools: Read, Glob\n---\n\nReview carefully.\n",
    );
    write(
        &dir.path().join("skills/commit/SKILL.md"),
        "---\nname: commit\ndescription: Draft commits\n---\n\nWrite the commit.\n",
    );
    write(
        &dir.path().join("hooks.json"),
        r#"{"version":1,"hooks":{"afterFileEdit":[{"command":"cargo fmt"}],"sessionStart":[{"type":"prompt","prompt":"hi"}]}}"#,
    );
    write(
        &dir.path().join("mcp.json"),
        r#"{"mcpServers":{"srv":{"command":"x"}}}"#,
    );

    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.metadata["source"], "cursor");
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "rust");
    assert_eq!(config.rules[0].description.as_deref(), Some("Rust rules"));
    assert_eq!(config.rules[0].globs, vec!["*.rs", "*.toml"]);
    assert!(config.rules[0].always_apply);
    assert_eq!(config.rules[0].body, "Use cargo.\n");
    assert!(config.rules[0].metadata["sourcePath"].is_string());

    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "review");

    assert_eq!(config.agents.len(), 1);
    assert_eq!(config.agents[0].name, "reviewer");
    assert_eq!(config.agents[0].tools, vec!["Read", "Glob"]);

    assert_eq!(config.skills.len(), 1);
    assert_eq!(config.skills[0].name, "commit");

    assert_eq!(config.hooks.len(), 2);
    let edit = config
        .hooks
        .iter()
        .find(|h| h.event == "AfterFileEdit")
        .unwrap();
    assert_eq!(edit.command.as_deref(), Some("cargo fmt"));
    assert_eq!(edit.kind, HookKind::Command);
    let start = config
        .hooks
        .iter()
        .find(|h| h.event == "SessionStart")
        .unwrap();
    assert_eq!(start.kind, HookKind::Prompt);
    assert_eq!(start.prompt.as_deref(), Some("hi"));
    assert_eq!(start.metadata["sourceEvent"], "sessionStart");

    assert_eq!(config.mcp_servers["srv"], json!({ "command": "x" }));
}

#[test]
fn missing_dirs_and_files_yield_empty_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.rules, vec![]);
    assert_eq!(config.commands, vec![]);
    assert_eq!(config.agents, vec![]);
    assert_eq!(config.skills, vec![]);
    assert_eq!(config.hooks, vec![]);
    assert_eq!(config.mcp_servers, serde_json::Map::new());
}

#[test]
fn malformed_hooks_json_and_non_object_mcp_degrade_to_empty() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("hooks.json"), "not json");
    write(&dir.path().join("mcp.json"), "[1]");
    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.hooks, vec![]);
    assert_eq!(config.mcp_servers, serde_json::Map::new());
}

#[test]
fn write_round_trips_rules_hooks_and_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let mut metadata = serde_json::Map::new();
    metadata.insert("custom".into(), json!("kept"));
    let config = AgentConfig {
        skills: Vec::new(),
        rules: vec![ConfigRule {
            name: "style".into(),
            description: Some("Style".into()),
            body: "Body text.\n".into(),
            globs: vec!["*.ts".into()],
            always_apply: true,
            kind: sepia_core::agent_config::RuleKind::Rule,
            metadata: Value::Object(metadata),
        }],
        commands: Vec::new(),
        hooks: vec![ConfigHook {
            event: "UserPromptSubmit".into(),
            matcher: None,
            kind: HookKind::Command,
            command: Some("echo hi".into()),
            prompt: None,
            timeout_sec: Some(10.0),
            fail_closed: Some(true),
            loop_limit: None,
            metadata: Value::Null,
        }],
        agents: Vec::new(),
        mcp_servers: serde_json::Map::from_iter([("srv".into(), json!({ "command": "x" }))]),
        metadata: Value::Null,
    };
    let actions = config::write(&config, dir.path()).unwrap();
    assert!(actions.iter().all(|a| a.action == WriteActionKind::Wrote));

    // rules land as .md with frontmatter
    let rule = std::fs::read_to_string(dir.path().join("rules/style.md")).unwrap();
    assert!(rule.contains("description: Style"));
    assert!(rule.contains("globs: \"*.ts\""));
    assert!(rule.contains("alwaysApply: true"));
    assert!(rule.contains("custom: kept"));
    assert!(rule.contains("Body text."));

    // hooks merge into a versioned hooks.json
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("hooks.json")).unwrap())
            .unwrap();
    assert_eq!(
        hooks,
        json!({
            "version": 1,
            "hooks": {
                "beforeSubmitPrompt": [
                    { "command": "echo hi", "timeout": 10, "failClosed": true }
                ]
            }
        })
    );

    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("mcp.json")).unwrap())
            .unwrap();
    assert_eq!(mcp["mcpServers"]["srv"], json!({ "command": "x" }));

    // and reading it back is lossless on the fields that matter
    let back = config::read(dir.path()).unwrap();
    assert_eq!(back.rules.len(), 1);
    assert_eq!(back.rules[0].globs, vec!["*.ts"]);
    assert!(back.rules[0].always_apply);
    assert_eq!(back.hooks.len(), 1);
    assert_eq!(back.hooks[0].event, "UserPromptSubmit");
    assert_eq!(back.hooks[0].timeout_sec, Some(10.0));
}
