#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `CursorRepository.test.ts` / `CursorRepositoryExtra.test.ts` ports plus
//! the shared `SessionRepository` contract and the committed golden
//! fixtures under `crates/sepia-testkit/fixtures/cursor/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use sepia_core::domain::{MessageNode, Role, Session};
use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_core::wire::session_to_json;
use sepia_driver_cursor::CursorStore;
use sepia_driver_cursor::cursor;
use sepia_testkit::{assert_json_eq, fixture_dir, materialize_store};
use serde_json::{Value, json};

fn make_node(
    node_id: i64,
    parent_node_id: Option<i64>,
    role: Role,
    content: &str,
    metadata: Value,
) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id,
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

fn writable_session(id: &str, cwd: &str, nodes: Vec<MessageNode>) -> Session {
    Session {
        id: id.into(),
        title: "unused title".into(),
        working_directory: cwd.into(),
        backend_type: "windsurf".into(),
        agent_mode: "accept-edits".into(),
        model: "composer-1".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_300.0,
        main_chain_id: nodes.len() as i64 - 1,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: json!({ "source": "import" }),
        nodes,
        prompt_history: Vec::new(),
    }
}

fn saved_nodes() -> Vec<MessageNode> {
    let mut assistant = make_node(
        1,
        Some(0),
        Role::Assistant,
        "I'll look around.",
        Value::Null,
    );
    assistant.thinking = Some(sepia_core::domain::REDACTED_THINKING.into());
    assistant.tool_calls = vec![sepia_core::domain::ToolCall {
        id: "call-1".into(),
        name: "Glob".into(),
        arguments: json!({ "glob_pattern": "src/**" }),
        index: 0,
        kind: "function".into(),
        status: None,
        exit_code: None,
        duration_ms: None,
        locations: Vec::new(),
        diffs: Vec::new(),
    }];
    vec![
        make_node(0, None, Role::User, "explore the repo", Value::Null),
        assistant,
    ]
}

fn write_transcript(
    root: &Path,
    slug: &str,
    chat_id: &str,
    lines: &[Value],
    mtime_ms: u64,
) -> PathBuf {
    let dir = root
        .join("projects")
        .join(slug)
        .join("agent-transcripts")
        .join(chat_id);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{chat_id}.jsonl"));
    let raw = lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&file, raw).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_millis(mtime_ms))
        .unwrap();
    file
}

fn transcript_line(role: &str, content: Value) -> Value {
    json!({ "role": role, "message": { "content": content } })
}

/* ------------------------------------------------------------------ */
/* fixture materialization                                             */
/* ------------------------------------------------------------------ */

fn set_mtime(path: &Path, ms: f64) {
    let mtime = UNIX_EPOCH + Duration::from_secs_f64(ms / 1000.0);
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(mtime);
    }
}

/// Fixture `store/` trees are checked in with SQL dumps (`store.sql`
/// alongside each `store.db` location, keeping binaries out of git) and
/// rely on file mtimes for transcript timestamps. Materialize: copy the
/// tree verbatim, replay each `*.sql` into its sibling, and re-stamp every
/// file with its source mtime so goldens are reproducible.
fn materialize_cursor_root(case_dir: &Path, scratch: &Path) -> PathBuf {
    let root = materialize_store(case_dir, scratch).unwrap();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            // Preserve the fixture's mtime — transcript timestamps derive
            // from it.
            let source = case_dir
                .join("store")
                .join(path.strip_prefix(&root).unwrap());
            if let Ok(m) = source.metadata().and_then(|m| m.modified()) {
                if let Ok(ms) = m
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64() * 1000.0)
                {
                    set_mtime(&path, ms);
                }
            }
            if name == "store.sql" {
                let db = path.with_file_name("store.db");
                let sql = std::fs::read_to_string(&path).unwrap();
                let conn = rusqlite::Connection::open(&db).unwrap();
                conn.execute_batch(&sql).unwrap();
                drop(conn);
                if let Ok(m) = path.metadata().and_then(|m| m.modified()) {
                    if let Ok(ms) = m
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs_f64() * 1000.0)
                    {
                        set_mtime(&db, ms);
                    }
                }
            }
        }
    }
    root
}

/* ------------------------------------------------------------------ */
/* contract + goldens                                                  */
/* ------------------------------------------------------------------ */

#[tokio::test]
async fn session_repository_contract() {
    let scratch = tempfile::tempdir().unwrap();
    let store = CursorStore::new(scratch.path().to_path_buf());
    sepia_testkit::assert_session_repository_contract(&store).await;
}

#[tokio::test]
async fn golden_fixtures() {
    let cases_dir = fixture_dir("cursor", "");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&cases_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    assert_eq!(
        entries.len(),
        3,
        "cursor fixtures changed — update the test"
    );

    for case_dir in entries {
        let case = case_dir.file_name().unwrap().to_string_lossy().to_string();
        let scratch = tempfile::tempdir().unwrap();
        let root = materialize_cursor_root(&case_dir, scratch.path());
        let store = CursorStore::new(root);

        let list: Vec<Value> = store
            .list()
            .await
            .unwrap()
            .iter()
            .map(|s| session_to_json(s).unwrap())
            .collect();
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(case_dir.join("list.json")).unwrap())
                .unwrap();
        assert_json_eq(&Value::Array(list), &expected, &format!("{case}/list.json"));

        for file in std::fs::read_dir(&case_dir).unwrap().flatten() {
            let name = file.file_name().to_string_lossy().to_string();
            let Some(id) = name
                .strip_prefix("export.")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            let session = store
                .get_by_id(id, None)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{case}: get_by_id({id}) returned none"));
            let expected: Value =
                serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
            assert_json_eq(
                &session_to_json(&session).unwrap(),
                &expected,
                &format!("{case}/export.{id}.json"),
            );
        }
    }
}

/* ------------------------------------------------------------------ */
/* repository-level ports                                              */
/* ------------------------------------------------------------------ */

#[tokio::test]
async fn missing_cursor_dir_lists_empty_and_reads_none() {
    let store = CursorStore::new(PathBuf::from("/nonexistent/cursor-dir"));
    assert_eq!(store.list().await.unwrap(), vec![]);
    assert_eq!(store.get_by_id("x", None).await.unwrap(), None);
    assert!(!store.has_session("x").await.unwrap());
}

#[tokio::test]
async fn list_picks_chat_store_over_transcript_and_reports_subagents() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    // a bare transcript-only chat
    write_transcript(
        root,
        "home-luis-proj",
        "chat-t",
        &[transcript_line(
            "user",
            json!([{ "type": "text", "text": "transcript only" }]),
        )],
        1_700_000_000_000,
    );
    // a chat dir whose store.db is real but has no meta row yet
    let chat_dir = root.join("chats").join("aabbcc").join("chat-c");
    std::fs::create_dir_all(&chat_dir).unwrap();
    rusqlite::Connection::open(chat_dir.join("store.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
    write_transcript(
        root,
        "home-luis-proj",
        "chat-c",
        &[transcript_line(
            "user",
            json!([{ "type": "text", "text": "chat has a store" }]),
        )],
        1_700_000_100_000,
    );
    // a subagent transcript under the chat — one junk line dropped, one
    // user line kept
    let sub_dir = root
        .join("projects")
        .join("home-luis-proj")
        .join("agent-transcripts")
        .join("chat-c")
        .join("subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    std::fs::write(
        sub_dir.join("sub-1.jsonl"),
        "not json\n".to_string()
            + &serde_json::to_string(&transcript_line(
                "user",
                json!([{ "type": "text", "text": "sub work" }]),
            ))
            .unwrap()
            + "\n",
    )
    .unwrap();

    let sessions = store_list(root).await;
    // chat-c's store beats its transcript; the subagent links to chat-c
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"chat-c"));
    assert!(ids.contains(&"chat-t"));
    assert!(ids.contains(&"sub-1"));
    let sub = sessions.iter().find(|s| s.id == "sub-1").unwrap();
    assert_eq!(sub.parent_session_id.as_deref(), Some("chat-c"));
    let chat = sessions.iter().find(|s| s.id == "chat-c").unwrap();
    assert_eq!(chat.metadata["store"], "chats");
}

async fn store_list(root: &Path) -> Vec<Session> {
    CursorStore::new(root.to_path_buf()).list().await.unwrap()
}

#[tokio::test]
async fn broken_store_db_degrades_to_meta_only_listing() {
    let scratch = tempfile::tempdir().unwrap();
    let chat_dir = scratch.path().join("chats").join("aa").join("broken-1");
    std::fs::create_dir_all(&chat_dir).unwrap();
    std::fs::write(chat_dir.join("store.db"), b"not a sqlite file").unwrap();
    std::fs::write(
        chat_dir.join("meta.json"),
        r#"{"schemaVersion":1,"title":"meta title","cwd":"/w/place","createdAtMs":1700000000000}"#,
    )
    .unwrap();
    let store = CursorStore::new(scratch.path().to_path_buf());
    let sessions = store.list().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "broken-1");
    assert_eq!(sessions[0].title, "meta title");
    assert_eq!(sessions[0].working_directory, "/w/place");
    assert_eq!(sessions[0].nodes, vec![]);
}

#[tokio::test]
async fn unsafe_ids_are_rejected_and_delete_cleans_both_stores() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let store = CursorStore::new(root.clone());
    let session = writable_session("safe-1", "/tmp/spot", saved_nodes());
    store.save(&session).await.unwrap();

    // the chat landed under chats/<md5(cwd)>/<id>/ with both stores written
    let ws = cursor::workspace_hash_from_cwd("/tmp/spot");
    let chat_dir = root.join("chats").join(&ws).join("safe-1");
    assert!(chat_dir.join("store.db").exists());
    assert!(chat_dir.join("meta.json").exists());
    assert!(chat_dir.join("prompt_history.json").exists());
    let transcript = root
        .join("projects")
        .join("tmp-spot")
        .join("agent-transcripts")
        .join("safe-1")
        .join("safe-1.jsonl");
    assert!(transcript.exists());

    for id in ["../escape", "a/b", "a\\b", ".", "..", ""] {
        assert!(
            store
                .save(&writable_session(id, "/tmp/spot", vec![]))
                .await
                .is_err()
        );
        assert!(store.delete(id).await.is_err());
    }

    // rewrites land on the same dirs — the chats store for the id is found
    // under whatever workspace hash already holds it
    let mut moved = writable_session("safe-1", "/tmp/moved", saved_nodes());
    moved.title = "moved".into();
    store.save(&moved).await.unwrap();
    let stale = root
        .join("chats")
        .join(cursor::workspace_hash_from_cwd("/tmp/moved"));
    assert!(!stale.join("safe-1").exists());

    store.delete("safe-1").await.unwrap();
    assert!(!chat_dir.exists());
    assert!(!transcript.exists());
    assert!(!store.has_session("safe-1").await.unwrap());
    assert_eq!(store.get_by_id("safe-1", None).await.unwrap(), None);
}

#[tokio::test]
async fn save_writes_a_canonical_store_and_transcript_that_round_trip() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let store = CursorStore::new(root.clone());
    let session = writable_session("new-chat", "/tmp/spot", saved_nodes());
    store.save(&session).await.unwrap();

    let transcript = root
        .join("projects")
        .join("tmp-spot")
        .join("agent-transcripts")
        .join("new-chat")
        .join("new-chat.jsonl");
    assert!(transcript.exists());

    let read = store.get_by_id("new-chat", None).await.unwrap().unwrap();
    assert_eq!(read.id, "new-chat");
    assert_eq!(read.title, "unused title");
    assert_eq!(read.working_directory, "/tmp/spot");
    assert_eq!(read.backend_type, "cursor");
    assert_eq!(read.metadata["store"], "chats");
    assert_eq!(read.last_activity_at, 1_700_000_300.0);
    // the write side derives the prompt history from the user query
    let prompts: Vec<&str> = read
        .prompt_history
        .iter()
        .map(|p| p.content.as_str())
        .collect();
    assert_eq!(prompts, ["explore the repo"]);
    // nodes survive the store round-trip verbatim (embedded IR keeps the
    // fields the blob format can't hold, like tool-call status)
    let roles: Vec<Role> = read.nodes.iter().map(|n| n.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    assert_eq!(read.nodes[0].content, "explore the repo");
    assert_eq!(read.nodes[1].tool_calls[0].name, "Glob");
    assert_eq!(
        read.nodes[1].tool_calls[0].arguments,
        json!({ "glob_pattern": "src/**" })
    );
    assert_eq!(
        read.nodes[1].thinking.as_deref(),
        Some(sepia_core::domain::REDACTED_THINKING)
    );
}

#[tokio::test]
async fn save_lands_subagents_under_the_parent_chats_transcript_dir() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let store = CursorStore::new(root.clone());
    let mut parent = writable_session("parent-1", "/tmp/proj", saved_nodes());
    parent.parent_session_id = None;
    store.save(&parent).await.unwrap();

    let mut child = writable_session("agent-5", "/tmp/proj", saved_nodes());
    child.parent_session_id = Some("parent-1".into());
    store.save(&child).await.unwrap();

    let sub = root
        .join("projects")
        .join("tmp-proj")
        .join("agent-transcripts")
        .join("parent-1")
        .join("subagents")
        .join("agent-5.jsonl");
    assert!(sub.exists());
    // a subagent has no chats-store concept — nothing was written there
    assert_eq!(
        read_dir_ids(&root.join("chats")),
        vec!["parent-1".to_string()]
    );

    let read = store.get_by_id("agent-5", None).await.unwrap().unwrap();
    assert_eq!(read.parent_session_id.as_deref(), Some("parent-1"));
    assert_eq!(read.metadata["store"], "transcript");
}

/// Chat ids under `chats/<ws>/<id>` across all workspace-hash dirs.
fn read_dir_ids(dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for ws in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            if let Ok(chats) = std::fs::read_dir(ws) {
                ids.extend(
                    chats
                        .flatten()
                        .map(|c| c.file_name().to_string_lossy().to_string()),
                );
            }
        }
    }
    ids.sort();
    ids
}

#[tokio::test]
async fn save_reuses_the_recorded_slug_and_falls_back_when_unsafe() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let store = CursorStore::new(root.clone());
    let mut session = writable_session("t-1", "/tmp/somewhere", vec![]);
    session.metadata = json!({ "store": "transcript", "project": "existing-slug" });
    store.save(&session).await.unwrap();
    assert!(
        root.join("projects")
            .join("existing-slug")
            .join("agent-transcripts")
            .join("t-1")
            .join("t-1.jsonl")
            .exists()
    );

    let mut weird = writable_session("t-2", "/tmp/somewhere", vec![]);
    weird.metadata = json!({ "store": "transcript", "project": "bad/../slug" });
    store.save(&weird).await.unwrap();
    assert!(
        root.join("projects")
            .join("tmp-somewhere")
            .join("agent-transcripts")
            .join("t-2")
            .join("t-2.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn list_re_scans_when_the_store_changes_externally() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let store = CursorStore::new(root.to_path_buf());
    write_transcript(
        root,
        "home-x-proj",
        "chat-1",
        &[transcript_line(
            "user",
            json!([{ "type": "text", "text": "one" }]),
        )],
        1_700_000_000_000,
    );
    assert_eq!(store.list().await.unwrap().len(), 1);

    // an external write — a new chat dir + bumped transcript mtime — must
    // invalidate the cached list
    write_transcript(
        root,
        "home-x-proj",
        "chat-2",
        &[transcript_line(
            "user",
            json!([{ "type": "text", "text": "two" }]),
        )],
        1_700_000_500_000,
    );
    let sessions = store.list().await.unwrap();
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"chat-1"));
    assert!(ids.contains(&"chat-2"));
}

#[tokio::test]
async fn nodes_window_pages_within_the_session() {
    let scratch = tempfile::tempdir().unwrap();
    let store = CursorStore::new(scratch.path().to_path_buf());
    store
        .save(&writable_session("w-1", "/tmp/spot", saved_nodes()))
        .await
        .unwrap();
    let window = store
        .nodes_window(
            "w-1",
            &NodesWindowOptions {
                before: None,
                limit: Some(1),
                agent_id: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.total, 2);
    assert_eq!(window.nodes.len(), 1);
    assert_eq!(window.backend_type, "cursor");
    assert_eq!(window.tool_call_nodes.len(), 1);
    assert!(
        store
            .nodes_window("missing", &NodesWindowOptions::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn transcript_timestamp_persists_through_the_save_stamp() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let store = CursorStore::new(root.to_path_buf());
        store
            .save(&writable_session("t-9", "/tmp/spot", saved_nodes()))
            .await
            .unwrap();
        // last_activity_at lands on the transcript file's mtime
        let transcript = root
            .join("projects")
            .join("tmp-spot")
            .join("agent-transcripts")
            .join("t-9")
            .join("t-9.jsonl");
        let ms = transcript
            .metadata()
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;
        assert_eq!(ms, 1_700_000_300_000.0);
    });
}
