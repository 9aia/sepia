#![allow(clippy::unwrap_used, clippy::pedantic)]

//! config.rs write-path edge coverage: unsafe stems skip, all sections.

use sepia_core::agent_config::{
    AgentConfig, ConfigCommand, ConfigHook, ConfigRule, ConfigSkill, HookKind, RuleKind,
};
use serde_json::json;

#[test]
fn write_full_config_covers_all_sections() {
    let config = AgentConfig {
        skills: vec![ConfigSkill {
            name: "sk".into(),
            description: None,
            body: "b".into(),
            files: vec![],
            metadata: json!({}),
        }],
        rules: vec![
            ConfigRule {
                name: "ok-rule".into(),
                description: None,
                body: "x".into(),
                globs: vec![],
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: json!({}),
            },
            // Unsafe stem → the skipped-action arm in write_docs.
            ConfigRule {
                name: "..".into(),
                description: None,
                body: "x".into(),
                globs: vec![],
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: json!({}),
            },
            ConfigRule {
                name: "memory".into(),
                description: None,
                body: "mem".into(),
                globs: vec![],
                always_apply: false,
                kind: RuleKind::Instructions,
                metadata: json!({"source": "/tmp/AGENTS.md"}),
            },
        ],
        commands: vec![ConfigCommand {
            name: "c".into(),
            description: None,
            argument_hint: None,
            allowed_tools: vec![],
            model: None,
            body: "b".into(),
            metadata: json!({}),
        }],
        hooks: vec![ConfigHook {
            event: "PreToolUse".into(),
            matcher: None,
            kind: HookKind::Command,
            command: Some("x".into()),
            prompt: None,
            timeout_sec: None,
            fail_closed: None,
            loop_limit: None,
            metadata: json!({}),
        }],
        agents: vec![],
        mcp_servers: Default::default(),
        metadata: json!({}),
    };
    let dir = tempfile::tempdir().unwrap();
    let actions = sepia_driver_devin::config::write(&config, dir.path()).unwrap();
    assert!(
        actions
            .iter()
            .any(|a| matches!(a.action, sepia_core::agent_config::WriteActionKind::Skipped))
    );
    // Second pass: memory file + hooks merge onto existing files.
    let _ = sepia_driver_devin::config::write(&config, dir.path()).unwrap();
}

#[test]
fn write_surfaces_io_failures_at_every_stage() {
    use sepia_core::agent_config::{
        AgentConfig, ConfigCommand, ConfigHook, ConfigRule, ConfigSkill, HookKind, RuleKind,
    };
    use sepia_driver_devin::config;
    use serde_json::{Map, Value};

    let rule = ConfigRule {
        name: "r".into(),
        description: None,
        body: "x".into(),
        globs: vec![],
        always_apply: false,
        kind: RuleKind::Rule,
        metadata: Value::Null,
    };
    let full = AgentConfig {
        skills: vec![ConfigSkill {
            name: "s".into(),
            description: None,
            body: "x".into(),
            files: vec![],
            metadata: Value::Null,
        }],
        rules: vec![rule.clone()],
        commands: vec![ConfigCommand {
            name: "c".into(),
            description: None,
            body: "x".into(),
            argument_hint: None,
            allowed_tools: vec![],
            model: None,
            metadata: Value::Null,
        }],
        hooks: vec![ConfigHook {
            event: "PreToolUse".into(),
            matcher: None,
            kind: HookKind::Command,
            command: Some("x".into()),
            prompt: None,
            timeout_sec: None,
            fail_closed: None,
            loop_limit: None,
            metadata: Value::Null,
        }],
        agents: vec![],
        mcp_servers: Map::new(),
        metadata: Value::Null,
    };

    for blocker in ["skills", "rules", "workflows"] {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(blocker), "file").unwrap();
        assert!(config::write(&full, d.path()).is_err(), "{blocker}");
    }

    // AGENTS.md: owned rule merge produces text, then the write fails.
    let d = tempfile::tempdir().unwrap();
    let agents = d.path().join("AGENTS.md");
    std::fs::write(&agents, "keep").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    let owned = AgentConfig {
        rules: vec![ConfigRule {
            kind: RuleKind::Instructions,
            metadata: json!({ "sourcePath": agents.display().to_string() }),
            ..rule.clone()
        }],
        ..full.clone()
    };
    assert!(config::write(&owned, d.path()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    // hooks.v1.json unwritable.
    let d = tempfile::tempdir().unwrap();
    let hooks = d.path().join("hooks.v1.json");
    std::fs::write(&hooks, "{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o444)).unwrap();
    }
    assert!(config::write(&full, d.path()).is_err());
}

#[test]
fn owned_rule_merges_into_agents_md_and_writes() {
    use sepia_core::agent_config::{AgentConfig, ConfigRule, RuleKind};
    use sepia_driver_devin::config;
    use serde_json::Value;
    let d = tempfile::tempdir().unwrap();
    let agents = d.path().join("AGENTS.md");
    std::fs::write(&agents, "# Existing\n").unwrap();
    let config = AgentConfig {
        rules: vec![ConfigRule {
            name: "memory".into(),
            description: None,
            body: "Remember this.".into(),
            globs: vec![],
            always_apply: false,
            kind: RuleKind::Instructions,
            metadata: Value::Null,
        }],
        ..AgentConfig::default()
    };
    config::write(&config, d.path()).unwrap();
    assert!(
        std::fs::read_to_string(&agents)
            .unwrap()
            .contains("Remember this.")
    );
}
