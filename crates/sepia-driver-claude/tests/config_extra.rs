#![allow(clippy::unwrap_used, clippy::pedantic)]

//! config.rs write-path edge coverage: full-section writes, unsafe file
//! stems, memory-file owners, settings-sourced hooks, and mcp merges.

use sepia_core::agent_config::{
    AgentConfig, ConfigAgent, ConfigCommand, ConfigHook, ConfigRule, ConfigSkill, HookKind,
    RuleKind,
};
use serde_json::json;

fn full_config() -> AgentConfig {
    AgentConfig {
        skills: vec![ConfigSkill {
            name: "sk".into(),
            description: Some("d".into()),
            body: "b".into(),
            files: vec![],
            metadata: json!({}),
        }],
        rules: vec![
            ConfigRule {
                name: "cmd-rule".into(),
                description: Some("d".into()),
                body: "body".into(),
                globs: vec![],
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: json!({"source": "/tmp/CLAUDE.md"}),
            },
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
                name: "block-rule".into(),
                description: None,
                body: "rule body".into(),
                globs: vec![],
                always_apply: false,
                kind: RuleKind::Instructions,
                metadata: json!({}),
            },
        ],
        commands: vec![ConfigCommand {
            name: "c".into(),
            description: Some("d".into()),
            argument_hint: Some("h".into()),
            allowed_tools: vec!["Read".into()],
            model: Some("m".into()),
            body: "b".into(),
            metadata: json!({}),
        }],
        hooks: vec![
            ConfigHook {
                event: "PreToolUse".into(),
                matcher: None,
                kind: HookKind::Command,
                command: Some("x".into()),
                prompt: None,
                timeout_sec: None,
                fail_closed: None,
                loop_limit: None,
                metadata: json!({"source": "settings.json"}),
            },
            ConfigHook {
                event: "PostToolUse".into(),
                matcher: None,
                kind: HookKind::Command,
                command: Some("y".into()),
                prompt: None,
                timeout_sec: None,
                fail_closed: None,
                loop_limit: None,
                metadata: json!({"source": "hooks/other.json"}),
            },
        ],
        agents: vec![ConfigAgent {
            name: "a".into(),
            description: Some("d".into()),
            tools: vec!["Read".into()],
            model: Some("m".into()),
            body: "b".into(),
            metadata: json!({}),
        }],
        mcp_servers: [("srv".to_string(), json!({"command": "run"}))]
            .into_iter()
            .collect(),
        metadata: json!({}),
    }
}

#[test]
fn write_full_config_covers_all_sections() {
    let dir = tempfile::tempdir().unwrap();
    let actions = sepia_driver_claude::config::write(&full_config(), dir.path()).unwrap();
    assert!(!actions.is_empty());
    // Second write merges into the now-existing memory files + settings.
    let actions = sepia_driver_claude::config::write(&full_config(), dir.path()).unwrap();
    assert!(!actions.is_empty());
}

#[tokio::test]
async fn claude_store_tail_edges() {
    use sepia_core::storage::{NodesWindowOptions, SessionRepository};
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    // A non-UTF8 project dir name → `dir_name` continue arm (unix only).
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bad = projects.join(std::ffi::OsStr::from_bytes(b"bad-\xff-name"));
        std::fs::create_dir_all(&bad).unwrap();
    }
    // Two transcripts with decreasing mtimes → the `mtime > max` arm.
    let p = projects.join("w-proj");
    std::fs::create_dir_all(&p).unwrap();
    let t1 = p.join("aaa.jsonl");
    let t2 = p.join("bbb.jsonl");
    for (f, content) in [
        (
            &t1,
            "{\"type\":\"user\",\"message\":{\"content\":\"q1\"}}\n",
        ),
        (
            &t2,
            "{\"type\":\"user\",\"message\":{\"content\":\"q2\"}}\n",
        ),
    ] {
        std::fs::write(f, content).unwrap();
    }
    let store = sepia_driver_claude::ClaudeStore::new(projects.clone());
    let _ = store.projects_dir();
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 2);
    // nodes_window with `before` → the map_or arm.
    let id = &listed[0].id;
    let w = store
        .nodes_window(
            id,
            &NodesWindowOptions {
                limit: Some(1),
                before: Some(0),
                agent_id: None,
            },
        )
        .await
        .unwrap();
    let _ = w;
    // Rewind paths: a transcript where a removed node precedes a kept one
    // → the order-check error; and the already-truncated → Ok(0) arm are
    // exercised through the session.rewind wire in bin_serve; here the
    // store-level delete removes the file.
    store.delete(id).await.unwrap();
}
