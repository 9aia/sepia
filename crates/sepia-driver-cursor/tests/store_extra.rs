#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `CursorStore` edge cases — scan filtering, degraded sqlite shapes,
//! the embedded-IR splice, `nodes_window` paging and write-path failures.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sepia_core::domain::{MessageNode, Role, Session, ToolCall};
use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_driver_cursor::{CursorStore, cursor};
use serde_json::{Value, json};

fn node(node_id: i64, role: Role, content: &str, metadata: Value) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: node_id.checked_sub(1),
        role,
        content: content.into(),
        blocks: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        tool_name: None,
        thinking: None,
        thinking_signature: None,
        usage: None,
        model: None,
        request_id: None,
        finish_reason: None,
        tool_result: None,
        created_at: 1.0,
        metadata,
    }
}

fn session(id: &str, nodes: Vec<MessageNode>) -> Session {
    Session {
        id: id.into(),
        title: "t".into(),
        working_directory: "/w".into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1.0,
        last_activity_at: 2.0,
        main_chain_id: nodes.len() as i64 - 1,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: Value::Null,
        nodes,
        prompt_history: Vec::new(),
    }
}

fn chat_dir(root: &Path, ws: &str, id: &str) -> PathBuf {
    let dir = root.join("chats").join(ws).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn store_db(dir: &Path) -> Connection {
    let conn = Connection::open(dir.join("store.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
    )
    .unwrap();
    conn
}

fn meta_value(root_blob_id: &str) -> String {
    cursor::encode_store_meta(&cursor::StoreMetaWriteInput {
        agent_id: "cursor".into(),
        latest_root_blob_id: root_blob_id.into(),
        name: Some("chat".into()),
        ..cursor::StoreMetaWriteInput::default()
    })
}

#[tokio::test]
async fn scan_entries_skips_non_dirs_and_non_jsonl_subagents() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A file where a workspace-hash dir belongs.
    std::fs::create_dir_all(root.join("chats")).unwrap();
    std::fs::write(root.join("chats").join("not-a-workspace"), "x").unwrap();
    // A file where a chat dir belongs.
    std::fs::create_dir_all(root.join("chats").join("ws1")).unwrap();
    std::fs::write(root.join("chats").join("ws1").join("not-a-chat"), "x").unwrap();
    // A transcript chat dir whose subagents dir holds junk.
    let subs = root.join("projects/slug/agent-transcripts/chat1/subagents");
    std::fs::create_dir_all(&subs).unwrap();
    std::fs::write(subs.join("readme.txt"), "not a transcript").unwrap();
    std::fs::write(
        subs.join("sub-1.jsonl"),
        "{\"role\":\"user\",\"message\":{\"content\":[]}}",
    )
    .unwrap();

    let store = CursorStore::new(root.to_path_buf());
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "sub-1");
    assert_eq!(listed[0].parent_session_id.as_deref(), Some("chat1"));
}

#[tokio::test]
async fn store_summary_degrades_on_odd_meta_and_blob_shapes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // meta['0'] present but not TEXT → reads as absent.
    let chat = chat_dir(root, "ws", "meta-int");
    store_db(&chat)
        .execute("INSERT INTO meta VALUES ('0', 42)", [])
        .unwrap();
    // meta → root blob whose `data` is TEXT, not a BLOB → workspace None.
    let chat = chat_dir(root, "ws", "meta-text-blob");
    let conn = store_db(&chat);
    conn.execute("INSERT INTO meta VALUES ('0', ?1)", [meta_value("b1")])
        .unwrap();
    conn.execute("INSERT INTO blobs VALUES ('b1', 'text-data')", [])
        .unwrap();
    // `store.db` a directory → open fails → (None, None) summary.
    let chat = chat_dir(root, "ws", "store-dir");
    std::fs::create_dir_all(chat.join("store.db")).unwrap();

    let listed = CursorStore::new(root.to_path_buf()).list().await.unwrap();
    assert_eq!(listed.len(), 3);
}

#[tokio::test]
async fn checkpoint_blob_workspace_and_unknown_proto_fields_decode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let chat = chat_dir(root, "ws", "ckpt");
    let conn = store_db(&chat);
    conn.execute("INSERT INTO meta VALUES ('0', ?1)", [meta_value("root")])
        .unwrap();
    // Valid checkpoint (field-9 workspace) plus a field-5 wire-2 member
    // the decoder doesn't model — it must skip, not choke.
    let mut blob = cursor::encode_checkpoint(&cursor::CheckpointWriteInput {
        message_ids: Vec::new(),
        workspace: Some("file:///work/dir".into()),
        client: Some("cli".into()),
    });
    blob.extend_from_slice(&[0x2A, 0x02, 0x08, 0x01]);
    conn.execute("INSERT INTO blobs VALUES ('root', ?1)", [blob])
        .unwrap();

    let listed = CursorStore::new(root.to_path_buf()).list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].working_directory, "/work/dir");
}

#[tokio::test]
async fn get_by_id_filters_non_blob_rows_and_reads_odd_prompt_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let chat = chat_dir(root, "ws", "chat-1");
    let conn = store_db(&chat);
    conn.execute("INSERT INTO meta VALUES ('0', ?1)", [meta_value("root")])
        .unwrap();
    // Rows whose id/data storage classes don't match (TEXT id + BLOB
    // data is the only pair kept).
    conn.execute("INSERT INTO blobs VALUES (1, X'AA')", [])
        .unwrap();
    conn.execute("INSERT INTO blobs VALUES ('t1', 'text')", [])
        .unwrap();
    let blob = cursor::encode_checkpoint(&cursor::CheckpointWriteInput {
        message_ids: Vec::new(),
        workspace: Some("file:///w".into()),
        client: None,
    });
    conn.execute("INSERT INTO blobs VALUES ('root', ?1)", [blob])
        .unwrap();
    drop(conn);
    // prompt_history.json present but not a flat array.
    std::fs::write(chat.join("prompt_history.json"), r#"{"a":1}"#).unwrap();

    let store = CursorStore::new(root.to_path_buf());
    let session = store.get_by_id("chat-1", None).await.unwrap().unwrap();
    assert_eq!(session.id, "chat-1");
}

#[tokio::test]
async fn embedded_ir_splices_fields_the_store_cannot_hold() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = CursorStore::new(root.to_path_buf());
    assert_eq!(store.cursor_dir(), root);

    let mut user = node(0, Role::User, "hello", json!({"extra": "x"}));
    let mut tool = node(1, Role::Tool, "result-body", json!({"k": "v"}));
    tool.tool_call_id = Some("tc-1".into());
    tool.tool_name = Some("Read".into());
    tool.tool_result = Some(sepia_core::domain::ToolResultInfo {
        status: sepia_core::domain::ToolCallStatus::Success,
        exit_code: None,
        duration_ms: None,
    });
    let mut call_node = node(2, Role::Assistant, "", Value::Null);
    call_node.tool_calls = vec![ToolCall {
        id: "tc-1".into(),
        name: "Read".into(),
        arguments: json!({"path": "/f"}),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: Vec::new(),
        diffs: Vec::new(),
    }];
    user.metadata = json!({"extra": "x"});
    let mut s = session("s-embed", vec![user, tool, call_node]);
    s.prompt_history = vec![sepia_core::domain::PromptHistoryEntry {
        content: "hi".into(),
        timestamp: 1.0,
        is_shell: false,
    }];

    store.save(&s).await.unwrap();
    let got = store.get_by_id("s-embed", None).await.unwrap().unwrap();
    assert_eq!(got.id, "s-embed");
    assert_eq!(got.prompt_history.len(), 1);
    // The embed carried node fields the blob format cannot.
    assert!(got.nodes.iter().any(|n| n.role == Role::Tool));

    // A Cursor-side append moves latestRootBlobId — the embed is stale
    // and the session falls back to the native decode.
    let chat = store
        .cursor_dir()
        .join("chats")
        .join(cursor::workspace_hash_from_cwd("/w"))
        .join("s-embed");
    let conn = Connection::open(chat.join("store.db")).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO meta VALUES ('0', ?1)",
        [meta_value("other")],
    )
    .unwrap();
    drop(conn);
    // A fresh store sees the moved root and ignores the stale embed.
    let fresh = CursorStore::new(root.to_path_buf());
    let got = fresh.get_by_id("s-embed", None).await.unwrap().unwrap();
    assert_eq!(got.id, "s-embed");
}

#[tokio::test]
async fn nodes_window_pages_with_before_and_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let transcripts = root.join("projects/slug/agent-transcripts/chat-9");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::write(
        transcripts.join("chat-9.jsonl"),
        [
            serde_json::to_string(&json!({
                "role": "user",
                "message": {"content": [{"type": "text", "text": "q"}]}
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "role": "assistant",
                "message": {"content": [{"type": "text", "text": "a"}]}
            }))
            .unwrap(),
        ]
        .join("\n"),
    )
    .unwrap();

    let store = CursorStore::new(root.to_path_buf());
    let window = store
        .nodes_window(
            "chat-9",
            &NodesWindowOptions {
                limit: Some(1),
                before: Some(1),
                agent_id: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.total, 2);
    assert_eq!(window.nodes.len(), 1);
}

#[tokio::test]
async fn save_reports_store_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = CursorStore::new(root.to_path_buf());
    let s = || session("sid", vec![node(0, Role::User, "hi", Value::Null)]);

    // store.db is a directory → the write-open fails.
    let chat = chat_dir(root, "ws", "sid");
    std::fs::create_dir_all(chat.join("store.db")).unwrap();
    assert!(store.save(&s()).await.is_err());
    std::fs::remove_dir(chat.join("store.db")).unwrap();

    // A non-sqlite store.db opens lazily, then fails at the first pragma.
    std::fs::write(chat.join("store.db"), b"not a database").unwrap();
    assert!(store.save(&s()).await.is_err());
    std::fs::remove_file(chat.join("store.db")).unwrap();

    // blobs with an extra NOT NULL column → the insert fails.
    let conn = Connection::open(chat.join("store.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB, extra TEXT NOT NULL);
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
    )
    .unwrap();
    drop(conn);
    assert!(store.save(&s()).await.is_err());
    std::fs::remove_file(chat.join("store.db")).unwrap();

    // Same trick against the meta row insert.
    let conn = Connection::open(chat.join("store.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT, extra TEXT NOT NULL);",
    )
    .unwrap();
    drop(conn);
    assert!(store.save(&s()).await.is_err());
}

#[tokio::test]
async fn delete_removes_transcript_files_and_subagent_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = CursorStore::new(root.to_path_buf());

    // A bare file named like a chat id under agent-transcripts.
    let transcripts = root.join("projects/s1/agent-transcripts");
    std::fs::create_dir_all(transcripts.join("parent-1/subagents")).unwrap();
    std::fs::write(transcripts.join("flat-id"), "x").unwrap();
    std::fs::write(transcripts.join("parent-1/subagents/gone.jsonl"), "{}").unwrap();

    store.delete("flat-id").await.unwrap();
    assert!(!transcripts.join("flat-id").exists());

    store.delete("gone").await.unwrap();
    assert!(!transcripts.join("parent-1/subagents/gone.jsonl").exists());
}

#[tokio::test]
async fn transcript_tool_use_without_id_and_patch_without_directives() {
    // A `tool_use` content block with no `id` gets a synthesized one.
    let source = cursor::CursorTranscriptSource {
        id: "t".into(),
        project_slug: "w".into(),
        parent_session_id: None,
        mtime_ms: Some(1.0),
    };
    let raw = serde_json::to_string(&json!({
        "role": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Glob", "input": {}}]}
    }))
    .unwrap();
    let session = cursor::from_transcript_jsonl(&raw, &source);
    assert_eq!(session.nodes[0].tool_calls.len(), 1);
    assert!(
        session.nodes[0].tool_calls[0]
            .id
            .starts_with("cursor-tool-")
    );

    // A `***` line that names no known directive is not a patch op.
    let refs = cursor::tool_file_refs("ApplyPatch", &json!("*** Bogus directive\n*** End Patch"));
    assert!(refs.locations.is_empty());
}

#[tokio::test]
async fn meta_row_with_a_non_text_value_reads_as_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = CursorStore::new(root.to_path_buf());
    let s = session("s-meta", vec![node(0, Role::User, "hi", Value::Null)]);
    store.save(&s).await.unwrap();
    // A BLOB meta['0'] — BLOBs bypass the column's TEXT affinity, so
    // meta_row's non-Text arm runs (an INTEGER would be coerced).
    let chat = store
        .cursor_dir()
        .join("chats")
        .join(cursor::workspace_hash_from_cwd("/w"))
        .join("s-meta");
    let conn = Connection::open(chat.join("store.db")).unwrap();
    conn.execute("INSERT OR REPLACE INTO meta VALUES ('0', X'00FF')", [])
        .unwrap();
    drop(conn);
    // summary → store_summary → meta_row's non-Text arm.
    let got = store.summary("s-meta", None).await.unwrap();
    assert!(got.is_some());
    let got = store.get_by_id("s-meta", None).await.unwrap();
    assert!(got.is_some());
}

#[tokio::test]
async fn embedded_ir_with_more_nodes_than_the_native_decode() {
    // The embed's per-node alignment: a native-exhausted splice leaves
    // the extra node as-is, and a non-emitting node skips the overlay.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = CursorStore::new(root.to_path_buf());
    let s = session("s-splice", vec![node(0, Role::User, "hi", Value::Null)]);
    store.save(&s).await.unwrap();
    let chat = store
        .cursor_dir()
        .join("chats")
        .join(cursor::workspace_hash_from_cwd("/w"))
        .join("s-splice");
    let meta_path = chat.join("meta.sepia.json");
    let mut meta_json: Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    let nodes = meta_json["session"]["nodes"].as_array_mut().unwrap();
    let extra_emitting = {
        let mut n = nodes[0].clone();
        n["nodeId"] = json!(1);
        n["content"] = json!("extra");
        n
    };
    let silent = {
        let mut n = nodes[0].clone();
        n["nodeId"] = json!(2);
        n["content"] = json!("");
        n
    };
    nodes.push(extra_emitting);
    nodes.push(silent);
    std::fs::write(&meta_path, meta_json.to_string()).unwrap();
    let got = store.get_by_id("s-splice", None).await.unwrap().unwrap();
    assert_eq!(got.nodes.len(), 3);
}

#[tokio::test]
async fn a_legacy_embedded_sidecar_in_meta_json_still_reads() {
    // Pre-fix writes buried the IR inside Cursor's own `meta.json`; the
    // reader still honours it when the separate sidecar is absent.
    let tmp = tempfile::tempdir().unwrap();
    let store = CursorStore::new(tmp.path().to_path_buf());
    let s = session("s-legacy", vec![node(0, Role::User, "hi", Value::Null)]);
    store.save(&s).await.unwrap();
    let chat = store
        .cursor_dir()
        .join("chats")
        .join(cursor::workspace_hash_from_cwd("/w"))
        .join("s-legacy");

    let sidecar: Value =
        serde_json::from_str(&std::fs::read_to_string(chat.join("meta.sepia.json")).unwrap())
            .unwrap();
    let mut meta: Value =
        serde_json::from_str(&std::fs::read_to_string(chat.join("meta.json")).unwrap()).unwrap();
    meta["sepia"] = sidecar;
    std::fs::write(chat.join("meta.json"), meta.to_string()).unwrap();
    std::fs::remove_file(chat.join("meta.sepia.json")).unwrap();

    let got = store.get_by_id("s-legacy", None).await.unwrap().unwrap();
    assert_eq!(got.nodes.len(), 1);
    assert_eq!(got.nodes[0].content, "hi");
}
