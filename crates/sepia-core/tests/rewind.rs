#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_core::domain::{CheckpointRef, MessageNode, Role, Session, ToolCall};
use sepia_core::rewind::{RewindPlan, RewindTarget, plan_rewind, rewind_session};
use serde_json::{Value, json};

fn node(
    node_id: i64,
    role: Role,
    tool_calls: Vec<ToolCall>,
    tool_call_id: Option<&str>,
    created_at: Option<f64>,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: (node_id != 0).then_some(node_id - 1),
        role,
        content: format!("{role:?} {node_id}").to_lowercase(),
        blocks: vec![],
        tool_calls,
        tool_call_id: tool_call_id.map(str::to_string),
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: created_at.unwrap_or(1_700_000_000.0 + node_id as f64),
        metadata: Value::Null,
    }
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "edit".into(),
        arguments: json!({}),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![],
    }
}

fn session(nodes: Vec<MessageNode>, checkpoints: Vec<CheckpointRef>) -> Session {
    Session {
        id: "s1".into(),
        title: "Session".into(),
        working_directory: "/work".into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "test-model".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_000.0 + nodes.len() as f64,
        main_chain_id: nodes.len() as i64 - 1,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints,
        metadata: Value::Null,
        nodes,
        prompt_history: vec![],
    }
}

fn chat_session() -> Session {
    session(
        vec![
            node(0, Role::System, vec![], None, None),
            node(1, Role::User, vec![], None, None),
            node(2, Role::Assistant, vec![], None, None),
            node(3, Role::Tool, vec![], Some("call_1"), None),
            node(4, Role::User, vec![], None, None),
            node(5, Role::Assistant, vec![call("call_2")], None, None),
            node(6, Role::Tool, vec![], Some("call_2"), None),
            node(7, Role::User, vec![], None, None),
            node(8, Role::Assistant, vec![], None, None),
        ],
        vec![],
    )
}

fn ids(nodes: &[MessageNode]) -> Vec<i64> {
    nodes.iter().map(|n| n.node_id).collect()
}

#[test]
fn node_id_keeps_through_the_named_node_and_drops_the_tail() {
    let plan = plan_rewind(&chat_session(), &RewindTarget::NodeId(4)).unwrap();
    assert_eq!(plan.keep_count, 5);
    assert_eq!(ids(&plan.kept), [0, 1, 2, 3, 4]);
    assert_eq!(ids(&plan.removed), [5, 6, 7, 8]);
    assert_eq!(plan.removed_tool_call_ids, ["call_2"]);
}

#[test]
fn node_id_on_the_tail_is_a_noop() {
    let plan = plan_rewind(&chat_session(), &RewindTarget::NodeId(8)).unwrap();
    assert!(plan.removed.is_empty());
}

#[test]
fn unknown_node_id_fails() {
    let err = plan_rewind(&chat_session(), &RewindTarget::NodeId(42)).unwrap_err();
    assert!(err.contains("Unknown node: 42"));
}

#[test]
fn turns_one_drops_the_last_user_turn() {
    let plan = plan_rewind(&chat_session(), &RewindTarget::Turns(1)).unwrap();
    assert_eq!(ids(&plan.kept), [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn turns_two_drops_the_last_two_user_turns() {
    let plan = plan_rewind(&chat_session(), &RewindTarget::Turns(2)).unwrap();
    assert_eq!(ids(&plan.kept), [0, 1, 2, 3]);
}

#[test]
fn turns_past_the_start_rewinds_to_before_the_first_turn() {
    let plan = plan_rewind(&chat_session(), &RewindTarget::Turns(9)).unwrap();
    assert_eq!(ids(&plan.kept), [0]);
}

#[test]
fn turns_validates_count_and_needs_a_user_node() {
    assert!(plan_rewind(&chat_session(), &RewindTarget::Turns(0)).is_err());
    let no_users = session(
        vec![
            node(0, Role::System, vec![], None, None),
            node(1, Role::Assistant, vec![], None, None),
        ],
        vec![],
    );
    assert!(plan_rewind(&no_users, &RewindTarget::Turns(1)).is_err());
}

#[test]
fn checkpoint_resolves_to_last_node_at_or_before_its_time() {
    let s = session(
        vec![
            node(0, Role::User, vec![], None, Some(1_700_000_000.0)),
            node(1, Role::Assistant, vec![], None, Some(1_700_000_005.0)),
            node(2, Role::Assistant, vec![], None, Some(1_700_000_010.0)),
        ],
        vec![CheckpointRef {
            r#ref: "sha-1".into(),
            created_at: 1_700_000_005_500.0,
            run_count: None,
            kind: None,
        }],
    );
    let plan = plan_rewind(&s, &RewindTarget::Checkpoint("sha-1".into())).unwrap();
    assert_eq!(ids(&plan.kept), [0, 1]);
    assert_eq!(ids(&plan.removed), [2]);
}

#[test]
fn checkpoint_rejects_unknown_ref() {
    let s = session(
        vec![node(0, Role::User, vec![], None, None)],
        vec![CheckpointRef {
            r#ref: "sha-1".into(),
            created_at: 1.0,
            run_count: None,
            kind: None,
        }],
    );
    let err = plan_rewind(&s, &RewindTarget::Checkpoint("sha-9".into())).unwrap_err();
    assert!(err.contains("Unknown checkpoint ref"));
}

#[test]
fn removed_tool_call_ids_excludes_calls_a_kept_node_still_claims() {
    let s = session(
        vec![
            node(0, Role::Assistant, vec![call("shared")], None, None),
            node(1, Role::Tool, vec![], Some("shared"), None),
            node(2, Role::Assistant, vec![call("gone")], None, None),
        ],
        vec![],
    );
    let plan = plan_rewind(&s, &RewindTarget::NodeId(0)).unwrap();
    assert_eq!(plan.removed_tool_call_ids, ["gone"]);
}

#[test]
fn rewind_session_applies_the_cut_and_moves_tail_markers() {
    let s = chat_session();
    let plan = plan_rewind(&s, &RewindTarget::NodeId(4)).unwrap();
    let truncated = rewind_session(&s, &plan);
    assert_eq!(ids(&truncated.nodes), [0, 1, 2, 3, 4]);
    assert_eq!(truncated.last_activity_at, s.nodes[4].created_at);
    assert_eq!(truncated.main_chain_id, 4);
    assert_eq!(truncated.id, s.id);
    assert_eq!(truncated.title, s.title);
}

#[test]
fn rewind_session_to_empty_keeps_creation_stamp() {
    let s = chat_session();
    let plan = RewindPlan {
        keep_count: 0,
        kept: vec![],
        removed: s.nodes.clone(),
        removed_tool_call_ids: vec![],
    };
    let truncated = rewind_session(&s, &plan);
    assert!(truncated.nodes.is_empty());
    assert_eq!(truncated.last_activity_at, s.created_at);
    assert_eq!(truncated.main_chain_id, 0);
}
