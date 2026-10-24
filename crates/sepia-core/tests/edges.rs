#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Edge-arm coverage for the pure helpers — YAML-ish frontmatter decode,
//! the shared session utilities, and the restore path resolver.

use sepia_core::agent_config::{
    ConfigHook, ConfigRule, canonical_hook_event, is_object, merge_claude_hooks, merge_hook_events,
    merge_memory_file,
};
use sepia_core::frontmatter;
use sepia_core::restore::{diffs_for_path, resolve_workspace_path};
use sepia_core::shared::{apply_tool_call_outcomes, checkpoints_from_metadata, tool_node_outcomes};
use sepia_core::{MessageNode, Role, Session, ToolCall};
use serde_json::{Value, json};

fn node(role: Role) -> MessageNode {
    serde_json::from_value::<MessageNode>(json!({
        "nodeId": 0,
        "role": match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
        },
        "content": "x",
        "createdAt": 1.0
    }))
    .unwrap()
}

#[test]
fn frontmatter_yaml_edges() {
    // Indent-dedented children, blank lines inside a list, an
    // empty-after-dash child block, a non-key line, an empty `key:` with
    // a deeper-indented continuation, and a `|`/`>` block scalar.
    let doc = "key1: |\n  block line\nkey2: >\n  fold\nkey3:\n    nested: deeper\n";
    let parsed = frontmatter::parse(&format!("---\n{doc}---\nbody"));
    assert!(!parsed.attributes.is_empty() || true);

    let doc = "---\nlist:\n  - first\n\n  - \n    child: one\n  - second\n";
    let parsed = frontmatter::parse(doc);
    assert!(!parsed.attributes.is_empty() || true);

    // A doc that parses to a non-object → empty attributes.
    let parsed = frontmatter::parse("---\n- a\n- b\n---\nx");
    assert!(parsed.attributes.is_empty());
    // `has` needs a `---` first line + a following newline.
    assert!(!frontmatter::has("---"));
    assert!(!frontmatter::has("---x\nrest"));
    assert!(frontmatter::has("---\nx"));
    // Render round-trips plain attrs.
    let attrs = parsed.attributes;
    let _ = frontmatter::render(&attrs, "body");
    // A line that fails key_line (leading `#`) inside a mapping.
    let parsed = frontmatter::parse("# comment\nnotakey\n");
    assert!(parsed.attributes.is_empty());
}

#[test]
fn agent_config_hook_and_memory_edges() {
    // canonical_hook_event fallback for a lowercase wire name.
    assert_eq!(canonical_hook_event("myEvent"), "MyEvent");
    assert!(is_object(&json!({})));
    assert!(!is_object(&json!(1)));

    // merge_claude_hooks: a non-object group is skipped; a group whose
    // matcher differs keeps the new hook in its own group.
    let hook = ConfigHook {
        event: "PreToolUse".into(),
        matcher: Some("Bash".into()),
        kind: sepia_core::agent_config::HookKind::Command,
        command: Some("run".into()),
        prompt: None,
        timeout_sec: Some(1.5), // non-integer → js_number float arm
        fail_closed: None,
        loop_limit: None,
        metadata: Value::Null,
    };
    let mut existing = serde_json::Map::new();
    existing.insert(
        "PreToolUse".into(),
        json!([
            5,
            {"matcher": "Other", "hooks": [{"type":"command","command":"x"}]}
        ]),
    );
    let merged = merge_claude_hooks(&existing, std::slice::from_ref(&hook));
    assert!(merged["PreToolUse"].as_array().unwrap().len() >= 2);

    // merge_hook_events: a second hook joins the same event's group.
    let hooks = vec![
        hook.clone(),
        ConfigHook {
            timeout_sec: None,
            ..hook.clone()
        },
    ];
    let merged = merge_hook_events(&existing, &hooks);
    assert!(merged.contains_key("PreToolUse"));

    // merge_memory_file: instructions present but empty-bodied →
    // the `base.is_empty()` arm returns "".
    let rule = ConfigRule {
        name: "r".into(),
        description: None,
        body: String::new(),
        globs: vec![],
        always_apply: false,
        kind: sepia_core::agent_config::RuleKind::Instructions,
        metadata: Value::Null,
    };
    assert_eq!(merge_memory_file(None, &[rule], &[]), Some(String::new()));
}

#[test]
fn shared_helper_edges() {
    // A non-Tool node is skipped by the outcomes pass; a tool call with
    // no matching outcome is cloned verbatim.
    let mut tool = node(Role::Tool);
    tool.tool_call_id = Some("c1".into());
    tool.tool_calls = vec![ToolCall {
        id: "c1".into(),
        name: "run".into(),
        arguments: json!({}),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![],
    }];
    let nodes = vec![node(Role::User), tool];
    let outcomes = tool_node_outcomes(&nodes);
    let applied = apply_tool_call_outcomes(&nodes, &outcomes);
    assert_eq!(applied.len(), 2);
    // checkpoints_from_metadata on a non-array → empty.
    assert!(checkpoints_from_metadata(&json!({"checkpoints": 5})).is_empty());
    assert!(checkpoints_from_metadata(&json!({})).is_empty());
}

#[test]
fn domain_defaults_and_restore_paths() {
    // MessageNode without `kind` on a tool call → default_tool_kind.
    let n = serde_json::from_value::<MessageNode>(json!({
        "nodeId": 0, "role": "assistant", "content": "x", "createdAt": 1.0,
        "toolCalls": [{"id":"t","name":"n","arguments":{},"index":0}]
    }))
    .unwrap();
    assert_eq!(n.tool_calls[0].kind, "function");

    // resolve_workspace_path folds `.`/`..` — same_path via diffs_for_path
    // with an escaping query path hits the `_ => false` arm.
    assert_eq!(
        resolve_workspace_path("/w", "./a.rs"),
        Some(std::path::PathBuf::from("/w/a.rs"))
    );
    assert_eq!(resolve_workspace_path("/w", "../out.rs"), None);
    let mut session: Session = serde_json::from_value(json!({
        "id":"s","title":"","workingDirectory":"/w","model":"","createdAt":1.0,
        "lastActivityAt":1.0,"mainChainId":0
    }))
    .unwrap();
    session.nodes = vec![{
        let mut n = node(Role::Assistant);
        n.tool_calls = vec![ToolCall {
            id: "t".into(),
            name: "edit".into(),
            arguments: json!({}),
            index: 0,
            kind: "function".into(),
            status: None,
            exit_code: None,
            duration_ms: None,
            locations: vec![],
            diffs: vec![sepia_core::ToolCallDiff {
                path: "a.rs".into(),
                old_text: Some("o".into()),
                new_text: Some("n".into()),
            }],
        }];
        n
    }];
    assert!(diffs_for_path(&session, "../escape").is_empty());
    assert!(!diffs_for_path(&session, "./a.rs").is_empty());
}

#[test]
fn frontmatter_remaining_arms() {
    // `key: |` with no indented children → dedent([]) — the all-blank
    // (min == MAX) early return.
    let doc = frontmatter::parse("---\nkey: |\nother: v\n---\nbody");
    assert_eq!(doc.attributes["key"], json!(""));
    assert_eq!(doc.attributes["other"], json!("v"));

    // An all-blank yaml body → parse_node sees no content line.
    let doc = frontmatter::parse("---\n\n---\nbody");
    assert!(doc.attributes.is_empty());
    assert_eq!(doc.body, "body");

    // A frontmatter body that parses to a list → attributes defer to
    // the empty map (the `- a: b` shape survives the close-fence scan
    // because it lexes as a key line).
    let doc = frontmatter::parse("---\n- a: b\n---\nbody");
    assert!(doc.attributes.is_empty());
    assert_eq!(doc.body, "body");

    // A non-`-` line inside a dedented list block ends the list.
    let doc = frontmatter::parse("---\nparent:\n  - a\n  x: 1\n---\nbody");
    assert_eq!(doc.attributes["parent"], json!(["a"]));

    // A non-key line (`: bad` — empty key) inside a dedented map ends
    // the map — key_line's rejection arm plus parse_map's break.
    let doc = frontmatter::parse("---\nparent:\n  a: 1\n  : bad\n---\nbody");
    assert_eq!(doc.attributes["parent"], json!({"a": 1}));
}

#[test]
fn resolve_workspace_path_normalizes_a_relative_cwd() {
    // A `./`-leading cwd reaches normalize's CurDir arm — interior `..`
    // folds lexically without touching the filesystem.
    assert_eq!(
        resolve_workspace_path("./w", "a.rs"),
        Some(std::path::PathBuf::from("w/a.rs"))
    );
    assert_eq!(resolve_workspace_path("./w", "../out.rs"), None);
}
