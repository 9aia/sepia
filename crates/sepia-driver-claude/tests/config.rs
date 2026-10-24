#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Config-store tests: `.claude` dir read/write — memory files, skills,
//! commands, agents, `settings.json`/`settings.local.json` hooks and
//! `.mcp.json` servers.

use std::path::Path;

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigHook, ConfigRule, ConfigSkill, HookKind, RuleKind,
    WriteActionKind,
};
use sepia_driver_claude::config;
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

fn command_hook(event: &str, command: &str, source: &str) -> ConfigHook {
    ConfigHook {
        event: event.into(),
        matcher: None,
        kind: HookKind::Command,
        command: Some(command.into()),
        prompt: None,
        timeout_sec: None,
        fail_closed: None,
        loop_limit: None,
        metadata: json!({ "source": source }),
    }
}

#[test]
fn read_write_round_trip_is_stable_and_lossless() {
    // A project `.claude` dir: the parent CLAUDE.md is the `claude-project`
    // memory; hooks come from both settings files.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("proj/.claude");
    write(&dir.join("CLAUDE.md"), "Be nice.\n");
    write(&root.path().join("proj/CLAUDE.md"), "Project notes.\n");
    write(
        &dir.join("settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"done.sh"}]}]}}"#,
    );
    write(
        &dir.join("settings.local.json"),
        r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"fmt.sh","timeout":30}]}]}}"#,
    );
    write(
        &dir.join("commands/review.md"),
        "---\ndescription: Review\nargument-hint: \"[files]\"\n---\n\nReview it.\n",
    );
    write(
        &dir.join("agents/reviewer.md"),
        "---\nname: reviewer\ntools: Read, Glob\nmodel: sonnet\n---\n\nReview carefully.\n",
    );
    write(
        &dir.join("skills/commit/SKILL.md"),
        "---\nname: commit\ndescription: Draft commits\n---\n\nWrite the commit.\n",
    );
    write(&dir.join("skills/commit/helper.sh"), "#!/bin/sh\n");
    write(
        &root.path().join("proj/.mcp.json"),
        r#"{"mcpServers":{"srv":{"command":"x"}}}"#,
    );

    let config = config::read(&dir).unwrap();
    let actions = config::write(&config, &dir).unwrap();

    // Native memory files write back verbatim — no doubling, no `##` section.
    assert_eq!(
        std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
        "Be nice.\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("proj/CLAUDE.md")).unwrap(),
        "Project notes.\n"
    );
    for file in [dir.join("CLAUDE.md"), root.path().join("proj/CLAUDE.md")] {
        let action = actions
            .iter()
            .find(|a| a.path == file.display().to_string())
            .unwrap_or_else(|| panic!("no action for {}", file.display()));
        assert_eq!(
            action.action,
            WriteActionKind::Unchanged,
            "{}",
            file.display()
        );
    }

    // Hooks stayed in the file they came from.
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["model"], json!("opus"));
    assert!(settings["hooks"].get("PostToolUse").is_none());
    let local: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("settings.local.json")).unwrap())
            .unwrap();
    assert_eq!(
        local["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        json!("fmt.sh")
    );

    // Re-read reproduces the same IR — the write was lossless.
    let back = config::read(&dir).unwrap();
    let bodies: Vec<&str> = back.rules.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["Be nice.", "Project notes."]);
    assert_eq!(back.hooks.len(), 2);
    assert_eq!(back.skills[0].files.len(), 1);
    assert_eq!(back.agents[0].tools, vec!["Read", "Glob"]);
    assert_eq!(back.mcp_servers["srv"], json!({ "command": "x" }));
}

#[test]
fn foreign_rules_collapse_into_the_managed_claude_md_block() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("CLAUDE.md"), "Keep me.\n");
    let config = AgentConfig {
        rules: vec![
            // A scoped rule (e.g. imported from Cursor).
            ConfigRule {
                globs: vec!["*.rs".into()],
                always_apply: false,
                ..rule("style", RuleKind::Rule, "Use cargo.")
            },
            // An instructions rule with no provenance in this dir.
            rule("agents", RuleKind::Instructions, "Devin memory."),
        ],
        ..AgentConfig::default()
    };
    config::write(&config, dir.path()).unwrap();
    let memory = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
    assert!(memory.contains("Keep me."));
    assert!(memory.contains("<!-- sepia:rules -->"));
    assert!(memory.contains("## style"));
    assert!(memory.contains("> Applies to: *.rs"));
    assert!(memory.contains("## agents"));
}

#[test]
fn hooks_partition_by_metadata_source_and_note_dropped_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut cursor_hook = command_hook("AfterFileEdit", "fmt", "settings.json");
    cursor_hook.fail_closed = Some(true);
    cursor_hook.loop_limit = Some(3.0);
    let config = AgentConfig {
        hooks: vec![
            command_hook("Stop", "a.sh", "settings.local.json"),
            cursor_hook,
        ],
        ..AgentConfig::default()
    };
    let actions = config::write(&config, dir.path()).unwrap();

    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).unwrap())
            .unwrap();
    let events: Vec<&str> = settings["hooks"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(events, ["AfterFileEdit"]); // cursor-only events keep their spelling
    let local: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("settings.local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        local["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("a.sh")
    );

    // The dropped cursor-only fields are reported on the settings action.
    let settings_action = actions
        .iter()
        .find(|a| a.path.ends_with("settings.json"))
        .unwrap();
    assert!(
        settings_action
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("failClosed")
    );
}

#[test]
fn write_to_a_fresh_dir_creates_all_surfaces() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        rules: vec![rule("style", RuleKind::Rule, "Use bun.")],
        hooks: vec![command_hook("Stop", "done.sh", "settings.json")],
        ..AgentConfig::default()
    };
    let actions = config::write(&config, dir.path()).unwrap();
    assert!(actions.iter().all(|a| a.action == WriteActionKind::Wrote));
    let memory = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
    assert!(memory.contains("## style"));
    // mcp dir form (non-.claude dir name) → `<dir>/.mcp.json` when servers set
    let mut with_mcp = AgentConfig::default();
    with_mcp
        .mcp_servers
        .insert("srv".into(), json!({ "command": "x" }));
    config::write(&with_mcp, dir.path()).unwrap();
    let mcp: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(mcp["mcpServers"]["srv"], json!({ "command": "x" }));
}

#[test]
fn write_skips_unusable_stems_and_propagates_io_failures() {
    let dir = tempfile::tempdir().unwrap();
    let config = AgentConfig {
        commands: vec![ConfigCommand {
            name: "..".into(),
            description: None,
            body: "x".into(),
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        ..AgentConfig::default()
    };
    let actions = config::write(&config, dir.path()).unwrap();
    assert!(actions.iter().any(|a| a.action == WriteActionKind::Skipped));

    let full = AgentConfig {
        skills: vec![ConfigSkill {
            name: "s".into(),
            description: None,
            body: "x".into(),
            files: Vec::new(),
            metadata: Value::Null,
        }],
        commands: vec![ConfigCommand {
            name: "c".into(),
            description: None,
            body: "x".into(),
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            metadata: Value::Null,
        }],
        agents: vec![],
        mcp_servers: serde_json::Map::from_iter([("srv".into(), json!({}))]),
        ..AgentConfig::default()
    };
    for blocker in ["skills", "commands"] {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(blocker), "file").unwrap();
        assert!(config::write(&full, d.path()).is_err(), "{blocker}");
    }
    // CLAUDE.md merge produces text, then the write fails.
    let d = tempfile::tempdir().unwrap();
    let memory = d.path().join("CLAUDE.md");
    std::fs::write(&memory, "keep").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&memory, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    let owned = AgentConfig {
        rules: vec![ConfigRule {
            name: "mem".into(),
            description: None,
            body: "More".into(),
            globs: Vec::new(),
            always_apply: false,
            kind: sepia_core::agent_config::RuleKind::Instructions,
            metadata: serde_json::json!({ "sourcePath": memory.display().to_string() }),
        }],
        ..AgentConfig::default()
    };
    assert!(config::write(&owned, d.path()).is_err());
    // mcp.json exists but can't be rewritten.
    let d = tempfile::tempdir().unwrap();
    let mcp = d.path().join(".mcp.json");
    std::fs::write(&mcp, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&mcp, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    assert!(config::write(&full, d.path()).is_err());
}
