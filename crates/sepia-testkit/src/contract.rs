//! `assert_session_repository_contract` — the behavioral contract every
//! session-store implementation must satisfy. A driver's test is:
//!
//! ```ignore
//! #[tokio::test]
//! async fn contract() {
//!     let store = my_store(scratch.path()).await.unwrap();
//!     sepia_testkit::assert_session_repository_contract(&store).await;
//! }
//! ```

use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_core::{Role, Session};
use serde_json::{Value, json};

pub fn session(id: &str, title: &str, activity: f64) -> Session {
    Session {
        id: id.into(),
        title: title.into(),
        working_directory: "/contract".into(),
        backend_type: "contract".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: activity,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: vec![],
        metadata: Value::Null,
        nodes: vec![sepia_core::MessageNode {
            node_id: 0,
            parent_node_id: None,
            role: Role::User,
            content: format!("hello from {id}"),
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
        }],
        prompt_history: vec![],
    }
}

pub fn rich_session(id: &str) -> Session {
    let mut s = session(id, "rich", 2.0);
    s.metadata = json!({ "sepia/checkpoints": [{ "ref": "cp-1", "createdAt": 1.5 }] });
    let mut tool_node = s.nodes[0].clone();
    tool_node.node_id = 1;
    tool_node.role = Role::Assistant;
    tool_node.tool_calls = vec![sepia_core::ToolCall {
        id: "call-1".into(),
        name: "edit".into(),
        arguments: json!({ "file_path": "/contract/a.ts" }),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: vec![sepia_core::ToolCallLocation {
            path: "/contract/a.ts".into(),
            line: Some(3),
        }],
        diffs: vec![sepia_core::ToolCallDiff {
            path: "a.ts".into(),
            old_text: Some("before".into()),
            new_text: Some("after".into()),
        }],
    }];
    s.nodes.push(tool_node);
    s.prompt_history = vec![sepia_core::PromptHistoryEntry {
        content: "hello".into(),
        timestamp: 1_700_000_000_000.0,
        is_shell: false,
    }];
    s
}

/// Run the full contract against `store`. Panics on any violation — the
/// point is that tests read like spec assertions.
///
/// # Panics
/// On any contract violation.
pub async fn assert_session_repository_contract(store: &dyn SessionRepository) {
    // Unknown ids read as absent, never as errors.
    let missing = store.get_by_id("contract-missing", None).await;
    assert!(
        matches!(missing, Ok(None)),
        "get_by_id on an unknown id must be Ok(None), got {missing:?}"
    );
    assert!(
        !store.has_session("contract-missing").await.unwrap_or(true),
        "has_session on an unknown id must be false"
    );

    // Save → read round-trips the IR faithfully.
    store.save(&session("contract-a", "A", 1.0)).await.unwrap();
    store.save(&session("contract-b", "B", 2.0)).await.unwrap();
    let got = store.get_by_id("contract-a", None).await.unwrap().unwrap();
    assert_eq!(got.title, "A");
    assert_eq!(got.nodes.len(), 1);
    assert_eq!(got.nodes[0].content, "hello from contract-a");

    // has_session sees what save wrote.
    assert!(store.has_session("contract-a").await.unwrap());
    assert!(store.has_session("contract-b").await.unwrap());

    // list returns every saved session.
    let listed = store.list().await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"contract-a"),
        "list missing contract-a: {ids:?}"
    );
    assert!(
        ids.contains(&"contract-b"),
        "list missing contract-b: {ids:?}"
    );

    // summary returns the session without the node backlog.
    let summary = store.summary("contract-a", None).await.unwrap().unwrap();
    assert_eq!(summary.id, "contract-a");
    assert!(summary.nodes.is_empty(), "summary must not carry nodes");

    // Rich fields survive the round-trip.
    store.save(&rich_session("contract-rich")).await.unwrap();
    let rich = store
        .get_by_id("contract-rich", None)
        .await
        .unwrap()
        .unwrap();
    let call = &rich.nodes[1].tool_calls[0];
    assert_eq!(call.diffs[0].path, "a.ts");
    assert_eq!(call.locations[0].line, Some(3));
    assert_eq!(rich.prompt_history.len(), 1);

    // nodes_window pages the backlog.
    let window = store
        .nodes_window("contract-rich", &NodesWindowOptions::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.total, 2);
    let paged = store
        .nodes_window(
            "contract-rich",
            &NodesWindowOptions {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paged.nodes.len(), 1);
    let missing_window = store
        .nodes_window("contract-missing", &NodesWindowOptions::default())
        .await
        .unwrap();
    assert!(missing_window.is_none());

    // delete removes what it was given.
    store.delete("contract-b").await.unwrap();
    assert!(!store.has_session("contract-b").await.unwrap());
    let listed = store.list().await.unwrap();
    assert!(!listed.iter().any(|s| s.id == "contract-b"));
}
