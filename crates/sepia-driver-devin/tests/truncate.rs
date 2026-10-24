#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `session.rewind` truncation against a real writable store.

use sepia_core::rewind::{self, RewindTarget};
use sepia_core::storage::SessionRepository;
use sepia_driver::SessionTruncator;
use sepia_driver_devin::store::DevinStore;
use sepia_driver_devin::truncate::DevinTruncator;
use sepia_testkit::contract;

#[tokio::test]
async fn truncates_removed_nodes_and_updates_session_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sessions.db");
    let store = DevinStore::open(&db, false).unwrap();

    // Ten alternating user/assistant nodes = five turns.
    let mut session = contract::session("s1", "S1", 1_700_000_100.0);
    session.nodes = (0..10)
        .map(|i| sepia_core::MessageNode {
            node_id: i,
            parent_node_id: (i > 0).then_some(i - 1),
            role: if i % 2 == 0 {
                sepia_core::Role::User
            } else {
                sepia_core::Role::Assistant
            },
            content: format!("m{i}"),
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
            created_at: 1_700_000_000.0 + i as f64,
            metadata: serde_json::Value::Null,
        })
        .collect();
    session.main_chain_id = 9;
    store.save(&session).await.unwrap();

    let plan = rewind::plan_rewind(&session, &RewindTarget::Turns(2)).unwrap();
    let truncated = rewind::rewind_session(&session, &plan);

    let truncator = DevinTruncator::new(
        store.db_path().to_path_buf(),
        store.has_tool_call_state(),
        store.has_subagent_heads(),
    );
    truncator
        .truncate(&session, &plan, &truncated)
        .await
        .unwrap();

    // A fresh read sees only the kept nodes and the updated header.
    let after = DevinStore::open(&db, true).unwrap();
    let after_session = after.get_by_id("s1", None).await.unwrap().unwrap();
    // Turns(2) drops the last two user+assistant pairs (4 nodes of 10).
    assert_eq!(after_session.nodes.len(), 6);
    assert_eq!(after_session.main_chain_id, truncated.main_chain_id);
    assert!(
        after_session
            .nodes
            .iter()
            .all(|n| plan.removed.iter().all(|r| r.node_id != n.node_id))
    );
}

#[tokio::test]
async fn truncate_reports_open_failures_and_tool_state_deletes() {
    // db_path a directory → the open fails.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocked");
    std::fs::write(&blocker, "x").unwrap();
    let truncator = DevinTruncator::new(blocker.join("sessions.db"), false, false);
    let session = contract::session("s1", "S1", 1.0);
    let plan = rewind::RewindPlan {
        keep_count: 0,
        kept: vec![],
        removed: vec![],
        removed_tool_call_ids: vec![],
    };
    assert!(truncator.truncate(&session, &plan, &session).await.is_err());

    // Empty `removed` skips the node delete; a tool-call-id delete runs
    // only when the table exists.
    let db = dir.path().join("sessions.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, last_activity_at REAL, main_chain_id INTEGER);
         CREATE TABLE tool_call_state (session_id TEXT, tool_call_id TEXT);
         INSERT INTO sessions VALUES ('s1', 1.0, 0);
         INSERT INTO tool_call_state VALUES ('s1', 'tc-1');",
    )
    .unwrap();
    drop(conn);
    let truncator = DevinTruncator::new(db.clone(), true, false);
    let plan = rewind::RewindPlan {
        keep_count: 0,
        kept: vec![],
        removed: vec![],
        removed_tool_call_ids: vec!["tc-1".into()],
    };
    let mut truncated = contract::session("s1", "S1", 2.0);
    truncated.main_chain_id = 0;
    truncator
        .truncate(&session, &plan, &truncated)
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let left: i64 = conn
        .query_row("select count(*) from tool_call_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}
