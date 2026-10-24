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

#[test]
fn read_tolerates_odd_hooks_shapes_and_canonical_event_names() {
    let dir = tempfile::tempdir().unwrap();
    // No `hooks` key at all → empty.
    write(&dir.path().join("hooks.json"), r#"{"version":1}"#);
    assert!(config::read(dir.path()).unwrap().hooks.is_empty());

    // Non-array entries and non-object entries are skipped; a wire name
    // that is already canonical keeps no `sourceEvent` marker.
    write(
        &dir.path().join("hooks.json"),
        r#"{"version":1,"hooks":{"afterFileEdit":"nope","SessionStart":[{"command":"x"},"stray",{"type":"prompt","prompt":"p","loop_limit":3,"matcher":"*.rs","timeout":10.5,"failClosed":false}]}}"#,
    );
    let hooks = config::read(dir.path()).unwrap().hooks;
    assert_eq!(hooks.len(), 2);
    let start = hooks.iter().find(|h| h.event == "SessionStart").unwrap();
    assert_eq!(start.command.as_deref(), Some("x"));
    assert!(start.metadata.get("sourceEvent").is_none());
    let prompt = hooks.iter().find(|h| h.kind == HookKind::Prompt).unwrap();
    assert_eq!(prompt.loop_limit, Some(3.0));
    assert_eq!(prompt.matcher.as_deref(), Some("*.rs"));
    assert_eq!(prompt.timeout_sec, Some(10.5));
}

#[test]
fn write_emits_full_command_agent_and_hook_fields() {
    use sepia_core::agent_config::{ConfigAgent, ConfigCommand};
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        commands: vec![ConfigCommand {
            name: "review".into(),
            description: Some("Review PRs".into()),
            body: "Review it.\n".into(),
            argument_hint: Some("<pr>".into()),
            allowed_tools: vec!["Read".into(), "Glob".into()],
            model: Some("opus".into()),
            metadata: Value::Null,
        }],
        agents: vec![ConfigAgent {
            name: "helper".into(),
            description: Some("Helps".into()),
            body: "Help out.\n".into(),
            tools: vec!["Read".into()],
            model: Some("sonnet".into()),
            metadata: Value::Null,
        }],
        hooks: vec![ConfigHook {
            event: "SessionStart".into(),
            matcher: None,
            kind: HookKind::Command,
            command: Some("echo go".into()),
            prompt: None,
            timeout_sec: Some(10.5),
            fail_closed: None,
            loop_limit: Some(2.0),
            metadata: Value::Null,
        }],
        ..AgentConfig::default()
    };
    config::write(&config, dir.path()).unwrap();

    let command = std::fs::read_to_string(dir.path().join("commands/review.md")).unwrap();
    assert!(command.contains("description: Review PRs"));
    assert!(command.contains("argument-hint: \"<pr>\""));
    assert!(command.contains("allowed-tools"));
    assert!(command.contains("model: opus"));
    let agent = std::fs::read_to_string(dir.path().join("agents/helper.md")).unwrap();
    assert!(agent.contains("description: Helps"));
    assert!(agent.contains("tools"));
    assert!(agent.contains("model: sonnet"));
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("hooks.json")).unwrap())
            .unwrap();
    assert_eq!(hooks["hooks"]["sessionStart"][0]["loop_limit"], json!(2));
}

#[test]
fn write_marks_unusable_stems_and_propagates_io_errors() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        rules: vec![ConfigRule {
            name: "..".into(),
            description: None,
            body: "x".into(),
            globs: Vec::new(),
            always_apply: false,
            kind: sepia_core::agent_config::RuleKind::Rule,
            metadata: Value::Null,
        }],
        ..AgentConfig::default()
    };
    let actions = config::write(&config, dir.path()).unwrap();
    assert_eq!(actions[0].action, WriteActionKind::Skipped);

    // Every write stage surfaces its I/O error: make each target dir a
    // file in turn.
    let full = AgentConfig {
        rules: vec![ConfigRule {
            name: "r".into(),
            description: None,
            body: "x".into(),
            globs: Vec::new(),
            always_apply: false,
            kind: sepia_core::agent_config::RuleKind::Rule,
            metadata: Value::Null,
        }],
        commands: vec![sepia_core::agent_config::ConfigCommand {
            name: "c".into(),
            description: None,
            body: "x".into(),
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        agents: vec![sepia_core::agent_config::ConfigAgent {
            name: "a".into(),
            description: None,
            body: "x".into(),
            tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        skills: vec![sepia_core::agent_config::ConfigSkill {
            name: "s".into(),
            description: None,
            body: "x".into(),
            files: Vec::new(),
            metadata: Value::Null,
        }],
        hooks: vec![ConfigHook {
            event: "SessionStart".into(),
            matcher: None,
            kind: HookKind::Command,
            command: Some("echo".into()),
            prompt: None,
            timeout_sec: None,
            fail_closed: None,
            loop_limit: None,
            metadata: Value::Null,
        }],
        mcp_servers: serde_json::Map::from_iter([("s".into(), json!({}))]),
        metadata: Value::Null,
    };
    for blocker in ["rules", "skills", "commands", "agents"] {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(blocker), "a file, not a dir").unwrap();
        assert!(config::write(&full, d.path()).is_err(), "{blocker}");
    }
    // mcp.json exists but can't be rewritten.
    let d = tempfile::tempdir().unwrap();
    let mcp = d.path().join("mcp.json");
    std::fs::write(&mcp, "{}").unwrap();
    let mut perms = std::fs::metadata(&mcp).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o444);
    }
    std::fs::set_permissions(&mcp, perms).unwrap();
    assert!(config::write(&full, d.path()).is_err());
}

#[test]
fn write_keeps_the_mdc_extension_for_mdc_sourced_rules() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("rules/rust.mdc"),
        "---\ndescription: Rust\nalwaysApply: true\n---\n\nUse cargo.\n",
    );
    let config = config::read(dir.path()).unwrap();
    assert_eq!(config.rules.len(), 1);

    let actions = config::write(&config, dir.path()).unwrap();
    let rule_action = actions
        .iter()
        .find(|a| a.path.contains("rules/rust"))
        .unwrap();
    assert!(rule_action.path.ends_with("rules/rust.mdc"));
    assert_eq!(rule_action.action, WriteActionKind::Unchanged);
    // No `.md` twin that would re-read as a duplicate rule.
    assert!(!dir.path().join("rules/rust.md").exists());
    let back = config::read(dir.path()).unwrap();
    assert_eq!(back.rules.len(), 1);
    assert_eq!(back.rules[0].name, "rust");

    // Rules with no `.mdc` provenance still land as `.md`.
    let foreign = AgentConfig {
        rules: vec![ConfigRule {
            name: "fresh".into(),
            description: None,
            body: "new".into(),
            globs: Vec::new(),
            always_apply: false,
            kind: sepia_core::agent_config::RuleKind::Rule,
            metadata: Value::Null,
        }],
        ..AgentConfig::default()
    };
    config::write(&foreign, dir.path()).unwrap();
    assert!(dir.path().join("rules/fresh.md").exists());
    assert!(!dir.path().join("rules/fresh.mdc").exists());
}
