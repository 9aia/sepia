#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `ClineConfig.test.ts` ports — `.clinerules` (file and dir forms),
//! `workflows/` commands, and `cline_mcp_settings.json` merging.

use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigHook, ConfigRule, ConfigSkill, HookKind,
    RuleKind, WriteActionKind,
};
use sepia_driver_cline::config;
use serde_json::{Value, json};

#[test]
fn read_treats_a_clinerules_file_as_one_instructions_rule() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".clinerules"), "Always use bun.\n").unwrap();
    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].name, "clinerules");
    assert_eq!(config.rules[0].kind, RuleKind::Instructions);
    assert!(config.rules[0].always_apply);
    assert_eq!(config.rules[0].body, "Always use bun.");
}

#[test]
fn read_treats_a_clinerules_dir_as_rules_plus_workflows_commands() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".clinerules/workflows")).unwrap();
    std::fs::write(dir.path().join(".clinerules/style.md"), "No comments.\n").unwrap();
    std::fs::write(
        dir.path().join(".clinerules/docs.md"),
        "---\ndescription: doc rule\n---\nDocs.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".clinerules/workflows/review.md"),
        "---\ndescription: review\n---\nReview the diff.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cline_mcp_settings.json"),
        json!({ "mcpServers": { "fs": { "command": "fs-mcp" } } }).to_string(),
    )
    .unwrap();
    let config = config::read(dir.path()).unwrap();
    let mut names: Vec<&str> = config.rules.iter().map(|r| r.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["docs", "style"]);
    assert!(config.rules.iter().all(|r| r.always_apply));
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "review");
    assert_eq!(config.commands[0].body, "Review the diff.\n");
    assert_eq!(config.mcp_servers["fs"], json!({ "command": "fs-mcp" }));
}

#[test]
fn read_on_an_empty_workspace_yields_an_empty_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.rules, Vec::<ConfigRule>::new());
    assert_eq!(config.commands, Vec::<ConfigCommand>::new());
    assert_eq!(config.mcp_servers, serde_json::Map::new());
}

#[test]
fn write_emits_clinerules_files_workflows_merged_mcp_and_skipped_notes() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        skills: vec![ConfigSkill {
            name: "s".into(),
            description: None,
            body: String::new(),
            files: Vec::new(),
            metadata: Value::Null,
        }],
        rules: vec![
            ConfigRule {
                name: "style".into(),
                description: None,
                body: "no comments".into(),
                globs: Vec::new(),
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: Value::Null,
            },
            ConfigRule {
                name: "   ".into(),
                description: None,
                body: "bad stem".into(),
                globs: Vec::new(),
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: Value::Null,
            },
        ],
        commands: vec![ConfigCommand {
            name: "go".into(),
            description: None,
            body: "do it".into(),
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        hooks: vec![ConfigHook {
            event: "Stop".into(),
            matcher: None,
            kind: HookKind::Command,
            command: None,
            prompt: None,
            timeout_sec: None,
            fail_closed: None,
            loop_limit: None,
            metadata: Value::Null,
        }],
        agents: Vec::<ConfigAgent>::new(),
        mcp_servers: serde_json::Map::from_iter([("srv".into(), json!({ "command": "srv" }))]),
        metadata: Value::Null,
    };
    let actions = config::write(&config, dir.path()).unwrap();
    assert!(
        std::fs::read_to_string(dir.path().join(".clinerules/style.md"))
            .unwrap()
            .contains("no comments")
    );
    assert!(
        std::fs::read_to_string(dir.path().join(".clinerules/workflows/go.md"))
            .unwrap()
            .contains("do it")
    );
    let mcp: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("cline_mcp_settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(mcp["mcpServers"]["srv"], json!({ "command": "srv" }));
    let skipped: Vec<_> = actions
        .iter()
        .filter(|a| a.action == WriteActionKind::Skipped)
        .collect();
    assert_eq!(skipped.len(), 3); // bad stem, skills, hooks (agents list is empty → no note)
    assert!(
        skipped
            .iter()
            .any(|a| a.detail.as_deref().unwrap_or("").contains("skills"))
    );
    assert!(
        skipped
            .iter()
            .any(|a| a.detail.as_deref().unwrap_or("").contains("hooks"))
    );
}

#[test]
fn write_round_trips_through_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".clinerules/workflows")).unwrap();
    std::fs::write(dir.path().join(".clinerules/style.md"), "No comments.\n").unwrap();
    std::fs::write(dir.path().join(".clinerules/workflows/go.md"), "Do.\n").unwrap();
    let config = config::read(dir.path()).unwrap();
    let target = dir.path().join("out");
    config::write(&config, &target).unwrap();
    let back = config::read(&target).unwrap();
    let rule_names: Vec<&str> = back.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(rule_names, ["style"]);
    let command_names: Vec<&str> = back.commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(command_names, ["go"]);
}
