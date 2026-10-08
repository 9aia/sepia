#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_core::domain::{MessageNode, Role, Session, ToolCall, ToolCallStatus, ToolResultInfo};
use sepia_core::shared::*;
use sepia_core::storage::{REQUIRED_TABLES, needs_migration};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn node() -> MessageNode {
    MessageNode {
        node_id: 0,
        parent_node_id: None,
        role: Role::Assistant,
        content: String::new(),
        blocks: vec![],
        tool_calls: vec![],
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1_700_000_000.0,
        metadata: Value::Null,
    }
}

fn call() -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: "exec".into(),
        arguments: json!({ "command": "ls" }),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![],
    }
}

#[test]
fn tool_node_outcomes_collects_tool_results_by_call_id() {
    let mut n1 = node();
    n1.role = Role::User;
    n1.content = "go".into();
    let mut n2 = node();
    n2.tool_calls = vec![call()];
    let mut n3 = node();
    n3.role = Role::Tool;
    n3.tool_call_id = Some("c1".into());
    n3.tool_result = Some(ToolResultInfo {
        status: ToolCallStatus::Error,
        exit_code: Some(2),
        duration_ms: Some(9.0),
    });
    let mut n4 = node();
    n4.role = Role::Tool;
    let outcomes = tool_node_outcomes(&[n1, n2, n3, n4]);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes["c1"].status, ToolCallStatus::Error);
}

#[test]
fn apply_tool_call_outcomes_stamps_outcome_onto_call() {
    let mut n1 = node();
    n1.tool_calls = vec![call()];
    let mut n2 = node();
    n2.role = Role::Tool;
    n2.tool_call_id = Some("c1".into());
    n2.tool_result = Some(ToolResultInfo {
        status: ToolCallStatus::Success,
        exit_code: Some(0),
        duration_ms: Some(12.0),
    });
    let nodes = vec![n1, n2];
    let out = apply_tool_call_outcomes(&nodes, &tool_node_outcomes(&nodes));
    let tc = &out[0].tool_calls[0];
    assert_eq!(tc.status, Some(ToolCallStatus::Success));
    assert_eq!(tc.exit_code, Some(0));
    assert_eq!(tc.duration_ms, Some(12.0));
}

#[test]
fn empty_outcome_map_returns_nodes_untouched() {
    let mut n1 = node();
    n1.tool_calls = vec![call()];
    let nodes = vec![n1];
    let empty: std::collections::HashMap<String, ToolCallOutcome> = Default::default();
    let out = apply_tool_call_outcomes(&nodes, &empty);
    assert_eq!(out, nodes);
}

#[test]
fn canonical_import_defaults_are_usable() {
    let meta = default_session_metadata();
    assert!(!meta["response_dimensions"].as_array().unwrap().is_empty());
    assert!(
        serde_json::from_str::<Value>(&default_cogs_json())
            .unwrap()
            .is_array()
    );
}

#[test]
fn checkpoints_from_metadata_drops_malformed() {
    let meta = json!({
        SESSION_CHECKPOINTS_KEY: [
            { "ref": "cp1", "createdAt": 1.5, "runCount": 3, "kind": "file_history" },
            { "ref": "missing-createdAt" },
            "not-an-object",
            null
        ]
    });
    let cps = checkpoints_from_metadata(&meta);
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0].r#ref, "cp1");
    assert_eq!(cps[0].created_at, 1.5);
    assert_eq!(cps[0].run_count, Some(3));
    assert_eq!(cps[0].kind.as_deref(), Some("file_history"));
    assert!(checkpoints_from_metadata(&Value::Null).is_empty());
    assert!(checkpoints_from_metadata(&json!({ "other": true })).is_empty());
}

#[test]
fn project_dir_slugs_round_trip() {
    assert_eq!(decode_project_dir("-home-me-proj"), "/home/me/proj");
    assert_eq!(decode_project_dir("bare"), "/bare");
    assert_eq!(encode_project_dir("/home/me/proj"), "-home-me-proj");
    assert_eq!(encode_project_dir("/my dir/x"), "-my-dir-x");
}

#[test]
fn needs_migration_only_when_required_table_missing() {
    let all: BTreeSet<String> = REQUIRED_TABLES.iter().map(|s| s.to_string()).collect();
    assert!(!needs_migration(&all));
    assert!(needs_migration(&BTreeSet::from(["sessions".to_string()])));
    assert!(needs_migration(&BTreeSet::new()));
}

#[test]
fn minimal_session_decodes_with_defaults() {
    let s: Session = serde_json::from_value(json!({
        "id": "x",
        "title": "t",
        "workingDirectory": "/w",
        "model": "m",
        "createdAt": 1,
        "lastActivityAt": 2,
        "mainChainId": 0,
        "metadata": null
    }))
    .unwrap();
    assert_eq!(s.backend_type, "windsurf");
    assert_eq!(s.agent_mode, "accept-edits");
    assert_eq!(s.cogs_json, "[]");
    assert!(s.checkpoints.is_empty());
    assert_eq!(s.hidden, 0);
}
