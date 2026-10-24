#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `DevinStore` coverage for the paths the contract suite can't reach:
//! `:memory:` stamp, `subagent_heads`/`tool_call_state` schema halves,
//! malformed rows, missing tables, and the cold `nodes_window` SQL path
//! (the whole-session cache only answers once `get_by_id` has run).

use std::path::PathBuf;

use rusqlite::Connection;
use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_driver_devin::DevinStore;
use sepia_testkit::contract;

#[tokio::test]
async fn memory_store_stamp_and_list() {
    let store = DevinStore::open(std::path::Path::new(":memory:"), false).unwrap();
    store
        .save(&contract::session("m1", "M1", 1.0))
        .await
        .unwrap();
    // db_stamp: the :memory: early return, then a real list through it.
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn subagent_parents_and_tool_call_state_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sessions.db");
    {
        let store = DevinStore::open(&db, false).unwrap();
        store
            .save(&contract::session("s1", "S1", 1.0))
            .await
            .unwrap();
        store
            .save(&contract::session("child", "C", 2.0))
            .await
            .unwrap();
    }
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE subagent_heads(session_id TEXT, agent_id TEXT, chain_node_id INTEGER);
             CREATE TABLE tool_call_state(session_id TEXT, tool_call_id TEXT, tool_call_update_json TEXT);",
        )
        .unwrap();
        conn.execute("INSERT INTO subagent_heads VALUES ('s1', 'child', 0)", [])
            .unwrap();
        conn.execute_batch(
            "INSERT INTO tool_call_state VALUES
               ('s1', 'good', '{\"toolCallId\":\"good\",\"status\":\"completed\"}'),
               ('s1', 'noid', '{\"status\":\"completed\"}'),
               ('s1', 'bad', '{\"toolCallId\":\"bad\",\"status\":\"bogus\"}'),
               ('s1', 'junk', 'not json');",
        )
        .unwrap();
    }
    let store = DevinStore::open(&db, true).unwrap();
    assert!(store.has_tool_call_state());
    assert!(store.has_subagent_heads());
    // list() walks subagent_heads rows.
    let listed = store.list().await.unwrap();
    assert!(listed.iter().any(|s| s.id == "child"));
    // get_by_id walks tool_call_state rows — malformed ones skip.
    let session = store.get_by_id("s1", None).await.unwrap().unwrap();
    assert_eq!(session.id, "s1");
    // summary() → parent_session_id + session_row paths.
    let summary = store.summary("child", None).await.unwrap().unwrap();
    assert_eq!(summary.parent_session_id.as_deref(), Some("s1"));

    // Now remove the column the outcomes query needs — prepare fails
    // and outcomes degrade to empty instead of failing the session.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "ALTER TABLE tool_call_state RENAME TO tcs_old;
             CREATE TABLE tool_call_state(session_id TEXT, tool_call_id TEXT);",
        )
        .unwrap();
    }
    let store2 = DevinStore::open(&db, true).unwrap();
    let session = store2.get_by_id("s1", None).await.unwrap().unwrap();
    assert_eq!(session.id, "s1");
}

#[tokio::test]
async fn list_and_summary_fail_on_a_store_without_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sessions.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE unrelated(x INTEGER);")
            .unwrap();
    }
    // Read-only open: no migration, so `sessions` stays absent.
    let store = DevinStore::open(&db, true).unwrap();
    let err = store.list().await.unwrap_err();
    assert!(err.message.contains("Failed to list sessions"));
    // session_row's `.optional()` propagates the same query error.
    let err = store.summary("s1", None).await.unwrap_err();
    assert!(err.message.contains("Failed to read session"));
    let err = store.get_by_id("s1", None).await.unwrap_err();
    assert!(err.message.contains("Failed to read session"));
}

fn tool_call_node(node_id: i64) -> sepia_core::MessageNode {
    sepia_core::MessageNode {
        node_id,
        parent_node_id: Some(node_id - 1),
        role: sepia_core::Role::Assistant,
        content: "calling".into(),
        blocks: vec![],
        tool_calls: vec![sepia_core::ToolCall {
            id: format!("call-{node_id}"),
            name: "edit".into(),
            arguments: serde_json::json!({"file_path": "a.ts"}),
            index: 0,
            kind: "function".into(),
            status: None,
            exit_code: None,
            duration_ms: None,
            locations: vec![],
            diffs: vec![],
        }],
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1_700_000_000.0 + node_id as f64,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn nodes_window_cold_scan_and_fresh_index() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sessions.db");
    {
        let store = DevinStore::open(&db, false).unwrap();
        let mut session = contract::session("s1", "S1", 1.0);
        session.nodes = vec![
            sepia_core::MessageNode {
                tool_result: Some(sepia_core::ToolResultInfo {
                    status: sepia_core::ToolCallStatus::Success,
                    exit_code: Some(0),
                    duration_ms: None,
                }),
                role: sepia_core::Role::Tool,
                ..tool_call_node(0)
            },
            tool_call_node(1),
            tool_call_node(2),
            sepia_core::MessageNode {
                role: sepia_core::Role::Tool,
                ..tool_call_node(3)
            },
            tool_call_node(4),
        ];
        session.nodes[0].tool_calls.clear();
        session.nodes[3].tool_calls.clear();
        store.save(&session).await.unwrap();
    }
    // Fresh store: session_cache is cold so nodes_window runs SQL.
    let store = DevinStore::open(&db, true).unwrap();
    // Window covers nodes 0..3 — tool-call node 4 sits outside it and is
    // resolved through the scan; node 1 is already in the window.
    let window = store
        .nodes_window(
            "s1",
            &NodesWindowOptions {
                limit: None,
                before: Some(4),
                agent_id: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.nodes.len(), 4);
    // Second call: the just-built calls index is fresh — the pooled
    // `entry.nodes` arm serves node 4 again without rescanning.
    let window = store
        .nodes_window(
            "s1",
            &NodesWindowOptions {
                limit: Some(2),
                before: Some(4),
                agent_id: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.nodes.len(), 2);
    assert!(!window.tool_call_nodes.is_empty());
    // A session the stamp query can't find → None.
    assert!(
        store
            .nodes_window("ghost", &NodesWindowOptions::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn truncator_error_paths() {
    use sepia_core::rewind::{self, RewindTarget};
    use sepia_driver::SessionTruncator;
    use sepia_driver_devin::truncate::DevinTruncator;

    let dir = tempfile::tempdir().unwrap();
    // Connection::open on a directory fails.
    let truncator = DevinTruncator::new(dir.path().join("missing-dir"), false, false);
    let session = contract::session("s1", "S1", 1.0);
    let plan = rewind::plan_rewind(&session, &RewindTarget::Turns(1)).unwrap();
    let truncated = rewind::rewind_session(&session, &plan);
    let err = truncator
        .truncate(&session, &plan, &truncated)
        .await
        .unwrap_err();
    assert!(!err.is_empty());

    // A garbage (non-sqlite) db file: open + pragma succeed, BEGIN fails.
    let garbage = dir.path().join("garbage.db");
    std::fs::write(&garbage, b"not a database at all").unwrap();
    let truncator = DevinTruncator::new(garbage, false, false);
    let err = truncator
        .truncate(&session, &plan, &truncated)
        .await
        .unwrap_err();
    assert!(!err.is_empty());

    // Tables the truncator deletes from are present; `sessions` is not —
    // the UPDATE fails and the transaction rolls back.
    let partial = dir.path().join("partial.db");
    {
        let conn = Connection::open(&partial).unwrap();
        conn.execute_batch(
            "CREATE TABLE message_nodes(session_id TEXT, node_id INTEGER);
             CREATE TABLE subagent_heads(session_id TEXT, chain_node_id INTEGER);
             CREATE TABLE tool_call_state(session_id TEXT, tool_call_id TEXT);",
        )
        .unwrap();
    }
    let mut session = contract::session("s1", "S1", 1.0);
    session.nodes = (0..4)
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
            created_at: i as f64,
            metadata: serde_json::Value::Null,
        })
        .collect();
    session.main_chain_id = 3;
    let plan = rewind::plan_rewind(&session, &RewindTarget::Turns(1)).unwrap();
    let truncated = rewind::rewind_session(&session, &plan);
    // has_subagent_heads + has_tool_call_state → both DELETE blocks run
    // before the UPDATE fails.
    let truncator = DevinTruncator::new(partial, true, true);
    let err = truncator
        .truncate(&session, &plan, &truncated)
        .await
        .unwrap_err();
    assert!(err.contains("Failed to truncate session"));
}
