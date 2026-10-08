#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_core::agent_config::*;
use serde_json::{Map, Value, json};

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn config_to_json_from_json_round_trips_every_collection() {
    let config = AgentConfig {
        skills: vec![ConfigSkill {
            name: "pnpm".into(),
            description: Some("use pnpm".into()),
            body: "body".into(),
            files: vec![ConfigFile {
                path: "references/core.md".into(),
                content: "ref".into(),
            }],
            metadata: json!({ "argument-hint": "[x]", "sourcePath": "/s" }),
        }],
        rules: vec![
            ConfigRule {
                name: "style".into(),
                description: None,
                body: "rule body".into(),
                globs: vec!["*.ts".into()],
                always_apply: false,
                kind: RuleKind::Rule,
                metadata: Value::Null,
            },
            ConfigRule {
                name: "claude".into(),
                description: None,
                body: "memory".into(),
                globs: vec![],
                always_apply: true,
                kind: RuleKind::Instructions,
                metadata: Value::Null,
            },
        ],
        commands: vec![ConfigCommand {
            name: "review".into(),
            description: None,
            body: "review it".into(),
            argument_hint: Some("[files]".into()),
            allowed_tools: vec!["read".into(), "grep".into()],
            model: None,
            metadata: Value::Null,
        }],
        hooks: vec![ConfigHook {
            event: "PostToolUse".into(),
            matcher: Some("^edit$".into()),
            kind: HookKind::Command,
            command: Some("sh hook.sh".into()),
            prompt: None,
            timeout_sec: Some(30.0),
            fail_closed: None,
            loop_limit: None,
            metadata: json!({ "source": "settings.json" }),
        }],
        agents: vec![ConfigAgent {
            name: "reviewer".into(),
            description: Some("reviews".into()),
            body: "you review".into(),
            tools: vec!["read".into()],
            model: None,
            metadata: Value::Null,
        }],
        mcp_servers: obj(json!({ "github": { "command": "mcp-github", "args": ["--x"] } })),
        metadata: Value::Null,
    };
    let json = config_to_json(&config);
    assert_eq!(json["version"], json!(1));
    let back = config_from_json(&json).unwrap();
    assert_eq!(back.skills[0].name, "pnpm");
    assert_eq!(back.skills[0].files[0].path, "references/core.md");
    assert_eq!(back.rules[0].description, None);
    assert_eq!(back.rules[1].kind, RuleKind::Instructions);
    assert_eq!(back.commands[0].allowed_tools, ["read", "grep"]);
    assert_eq!(back.hooks[0].event, "PostToolUse");
    assert_eq!(back.agents[0].tools, ["read"]);
    assert_eq!(
        back.mcp_servers["github"],
        json!({ "command": "mcp-github", "args": ["--x"] })
    );
}

#[test]
fn config_from_json_applies_every_default() {
    let back = config_from_json(&json!({ "version": 1, "skills": [{ "name": "x" }] })).unwrap();
    assert_eq!(back.skills[0].body, "");
    assert!(back.skills[0].files.is_empty());
    assert!(back.skills[0].description.is_none());
    assert!(back.rules.is_empty());
    assert!(back.mcp_servers.is_empty());
}

#[test]
fn config_from_json_fills_wire_defaults_on_sparse_items() {
    let back = config_from_json(&json!({
        "rules": [{ "name": "r" }],
        "commands": [{ "name": "c" }],
        "hooks": [{ "event": "Stop" }],
        "agents": [{ "name": "a" }]
    }))
    .unwrap();
    assert!(back.commands[0].allowed_tools.is_empty());
    assert_eq!(back.hooks[0].kind, HookKind::Command);
    assert_eq!(back.rules[0].kind, RuleKind::Rule);
    assert!(back.agents[0].tools.is_empty());
    let empty = config_from_json(&json!({})).unwrap();
    assert!(empty.skills.is_empty());
    assert!(empty.mcp_servers.is_empty());
}

#[test]
fn config_from_json_rejects_non_config_payload() {
    assert!(config_from_json(&json!({ "version": 1, "skills": "nope" })).is_err());
}

#[test]
fn hook_event_names_map_between_canonical_cursor_and_claude() {
    assert_eq!(
        canonical_hook_event("beforeSubmitPrompt"),
        "UserPromptSubmit"
    );
    assert_eq!(canonical_hook_event("preToolUse"), "PreToolUse");
    assert_eq!(canonical_hook_event("sessionStart"), "SessionStart");
    assert_eq!(canonical_hook_event("UserPromptSubmit"), "UserPromptSubmit");
    assert_eq!(
        canonical_hook_event("beforeShellExecution"),
        "BeforeShellExecution"
    );
    assert_eq!(cursor_hook_event("PreToolUse"), "preToolUse");
    assert_eq!(cursor_hook_event("UserPromptSubmit"), "beforeSubmitPrompt");
    assert_eq!(cursor_hook_event("CustomEvent"), "customEvent");
    assert_eq!(claude_hook_event("PreToolUse"), "PreToolUse");
    assert_eq!(claude_hook_event("customEvent"), "CustomEvent");
}

#[test]
fn safe_file_stem_strips_separators_dots_extensions() {
    assert_eq!(safe_file_stem("name").as_deref(), Some("name"));
    assert_eq!(safe_file_stem("a/b\\c").as_deref(), Some("a-b-c"));
    assert_eq!(safe_file_stem(".hidden").as_deref(), Some("hidden"));
    assert_eq!(safe_file_stem("rule.md").as_deref(), Some("rule"));
    assert_eq!(safe_file_stem("two words").as_deref(), Some("two-words"));
    assert_eq!(safe_file_stem("..."), None);
    assert_eq!(safe_file_stem(""), None);
}

#[test]
fn string_list_accepts_comma_strings_and_lists() {
    assert_eq!(string_list(&json!("*.ts, *.tsx")), ["*.ts", "*.tsx"]);
    assert_eq!(string_list(&json!(["a", "b"])), ["a", "b"]);
    assert!(string_list(&Value::Null).is_empty());
    assert!(string_list(&json!("")).is_empty());
    assert!(string_list(&json!(3)).is_empty());
}

#[test]
fn hooks_from_claude_json_flattens_matcher_groups() {
    let settings = obj(json!({
        "permissions": { "allow": ["x"] },
        "hooks": {
            "PostToolUse": [{
                "matcher": "^edit$",
                "hooks": [
                    { "type": "command", "command": "fmt.sh", "timeout": 30 },
                    { "type": "prompt", "prompt": "check it" }
                ]
            }],
            "Stop": [{ "hooks": [{ "type": "command", "command": "done.sh" }] }],
            "Bad": ["not-a-group", { "hooks": "nope" }, { "hooks": [42] }]
        }
    }));
    let hooks = hooks_from_claude_json(&settings, "settings.json");
    assert_eq!(hooks.len(), 3);
    assert_eq!(hooks[0].event, "PostToolUse");
    assert_eq!(hooks[0].matcher.as_deref(), Some("^edit$"));
    assert_eq!(hooks[0].command.as_deref(), Some("fmt.sh"));
    assert_eq!(hooks[0].timeout_sec, Some(30.0));
    assert_eq!(hooks[1].kind, HookKind::Prompt);
    assert_eq!(hooks[1].prompt.as_deref(), Some("check it"));
    assert!(hooks[2].matcher.is_none());
    assert!(hooks_from_claude_json(&Map::new(), "x").is_empty());
}

#[test]
fn hooks_from_claude_json_accepts_bare_event_map() {
    let bare = obj(json!({
        "PostToolUse": [{ "matcher": "^edit$", "hooks": [{ "type": "command", "command": "fmt.py" }] }]
    }));
    let hooks = hooks_from_claude_json(&bare, "hooks.v1.json");
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, "PostToolUse");
    assert_eq!(hooks[0].metadata["source"], json!("hooks.v1.json"));
    let nested = obj(json!({ "env": { "X": "1" }, "hooks": bare }));
    assert_eq!(hooks_from_claude_json(&nested, "settings.json").len(), 1);
    let no_hooks = obj(json!({ "env": { "X": "1" }, "model": "opus" }));
    assert!(hooks_from_claude_json(&no_hooks, "settings.json").is_empty());
}

fn hook(event: &str, command: Option<&str>) -> ConfigHook {
    ConfigHook {
        event: event.into(),
        matcher: None,
        kind: HookKind::Command,
        command: command.map(str::to_string),
        prompt: None,
        timeout_sec: None,
        fail_closed: None,
        loop_limit: None,
        metadata: Value::Null,
    }
}

#[test]
fn merge_hook_events_merges_into_bare_event_map() {
    let existing = obj(json!({
        "Stop": [{ "hooks": [{ "type": "command", "command": "keep.sh" }] }]
    }));
    let merged = merge_hook_events(
        &existing,
        &[hook("Stop", Some("keep.sh")), hook("Stop", Some("new.sh"))],
    );
    let groups = merged["Stop"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["hooks"].as_array().unwrap().len(), 2);
}

#[test]
fn event_names_pass_empty_and_unknown_strings_through() {
    assert_eq!(claude_hook_event(""), "");
    assert_eq!(cursor_hook_event(""), "");
    assert_eq!(canonical_hook_event("customWire"), "CustomWire");
}

#[test]
fn merge_claude_hooks_appends_dedupes_and_preserves_keys() {
    let existing = obj(json!({
        "model": "opus",
        "hooks": {
            "PostToolUse": [{ "matcher": "^edit$", "hooks": [{ "type": "command", "command": "old.sh" }] }]
        }
    }));
    let merged = merge_claude_hooks(
        &existing,
        &[
            ConfigHook {
                event: "PostToolUse".into(),
                matcher: Some("^edit$".into()),
                kind: HookKind::Command,
                command: Some("old.sh".into()),
                prompt: None,
                timeout_sec: None,
                fail_closed: None,
                loop_limit: None,
                metadata: Value::Null,
            },
            ConfigHook {
                event: "PostToolUse".into(),
                matcher: Some("^edit$".into()),
                kind: HookKind::Command,
                command: Some("new.sh".into()),
                prompt: None,
                timeout_sec: Some(5.0),
                fail_closed: None,
                loop_limit: None,
                metadata: Value::Null,
            },
            hook("Stop", Some("done.sh")),
        ],
    );
    assert_eq!(merged["model"], json!("opus"));
    let groups = merged["hooks"].as_object().unwrap();
    let post = groups["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    let entries = post[0]["hooks"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1]["type"], json!("command"));
    assert_eq!(entries[1]["command"], json!("new.sh"));
    assert_eq!(entries[1]["timeout"].as_f64(), Some(5.0));
    assert_eq!(
        groups["Stop"],
        json!([{ "hooks": [{ "type": "command", "command": "done.sh" }] }])
    );
}

fn rule(name: &str, body: &str) -> ConfigRule {
    ConfigRule {
        name: name.into(),
        description: None,
        body: body.into(),
        globs: vec![],
        always_apply: false,
        kind: RuleKind::Rule,
        metadata: Value::Null,
    }
}

#[test]
fn rules_to_memory_file_appends_managed_block() {
    let mut scoped = rule("docs", "update docs");
    scoped.description = Some("doc rule".into());
    scoped.globs = vec!["docs/**".into()];
    let out = rules_to_memory_file(
        Some("# My memory\n\nkeep this\n"),
        &[rule("style", "use bun"), scoped],
    )
    .unwrap();
    assert!(out.contains("# My memory"));
    assert!(out.contains(RULES_BLOCK_BEGIN));
    assert!(out.contains("## style"));
    assert!(out.contains("> Applies to: docs/**"));
    assert!(out.contains(RULES_BLOCK_END));
    let again = rules_to_memory_file(Some(&out), &[rule("only", "new")]).unwrap();
    assert!(again.contains("## only"));
    assert!(!again.contains("## style"));
    assert!(again.contains("# My memory"));
}

#[test]
fn rules_to_memory_file_none_with_no_rules_and_no_block() {
    assert!(rules_to_memory_file(Some("# hi\n"), &[]).is_none());
    assert!(rules_to_memory_file(None, &[]).is_none());
    let fresh = rules_to_memory_file(None, &[rule("x", "b")]).unwrap();
    assert!(fresh.contains("## x"));
}

#[test]
fn rules_to_memory_file_empties_stale_block() {
    let stale = format!("pre\n\n{RULES_BLOCK_BEGIN}\n\n## old\n\n{RULES_BLOCK_END}\npost\n");
    let out = rules_to_memory_file(Some(&stale), &[]).unwrap();
    assert!(out.contains("pre"));
    assert!(out.contains("post"));
    assert!(!out.contains("## old"));
    assert!(out.contains(RULES_BLOCK_BEGIN));
}

#[test]
fn merge_mcp_servers_merges_and_preserves() {
    assert!(merge_mcp_servers(None, &Map::new()).is_none());
    let existing = obj(json!({ "disabled": false, "mcpServers": { "old": { "command": "old" } } }));
    let servers = obj(json!({ "new": { "command": "new", "args": ["a"] } }));
    let merged = merge_mcp_servers(Some(&existing), &servers).unwrap();
    assert_eq!(merged["disabled"], json!(false));
    assert_eq!(
        merged["mcpServers"],
        json!({ "old": { "command": "old" }, "new": { "command": "new", "args": ["a"] } })
    );
}

#[test]
fn skill_attributes_emits_name_description_drops_provenance() {
    let skill = ConfigSkill {
        name: "x".into(),
        description: Some("d".into()),
        body: String::new(),
        files: vec![],
        metadata: Value::String("not-an-object".into()),
    };
    let attrs = skill_attributes(&skill);
    assert_eq!(attrs["name"], json!("x"));
    assert_eq!(attrs["description"], json!("d"));
    let with_meta = ConfigSkill {
        metadata: json!({ "sourcePath": "/s", "extra": "keep" }),
        ..skill
    };
    let attrs = skill_attributes(&with_meta);
    assert_eq!(attrs["extra"], json!("keep"));
    assert!(attrs.get("sourcePath").is_none());
}
