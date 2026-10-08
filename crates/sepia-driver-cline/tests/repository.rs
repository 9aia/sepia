#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `ClineRepository.test.ts` ports — the repository surface over a real
//! temp data dir, plus the `truncate_session` refusal paths.
//!
//! Divergence from the TS repository: `save`/`delete` are writable here
//! (the driver declares `SessionWrite` and the shared contract requires
//! them); the TS repository is read-only.

use std::path::Path;

use sepia_core::domain::{MessageNode, Role, Session, StorageError};
use sepia_core::storage::SessionRepository;
use sepia_driver_cline::ClineStore;
use serde_json::{Value, json};

fn write_cline_session(data_dir: &Path, id: &str, manifest: &Value, messages: &Value) {
    let dir = data_dir.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut base_messages = json!({
        "version": 1,
        "sessionId": id,
        "messages": [
            { "id": "m0", "role": "user", "content": [{ "type": "text", "text": "first" }], "ts": 1000 },
            { "id": "m1", "role": "assistant", "content": [{ "type": "text", "text": "answer" }], "ts": 2000 },
        ],
    });
    if let (Some(b), Some(extra)) = (base_messages.as_object_mut(), messages.as_object()) {
        for (k, v) in extra {
            b.insert(k.clone(), v.clone());
        }
    }
    std::fs::write(
        dir.join(format!("{id}.messages.json")),
        base_messages.to_string(),
    )
    .unwrap();
    let mut base_manifest = json!({
        "version": 1,
        "session_id": id,
        "cwd": "/work",
        "started_at": "2026-01-01T00:00:00.000Z",
        "ended_at": "2026-01-01T00:00:05.000Z",
        "status": "completed",
        "model": "cline-pass/glm-5-2",
    });
    if let (Some(b), Some(extra)) = (base_manifest.as_object_mut(), manifest.as_object()) {
        for (k, v) in extra {
            b.insert(k.clone(), v.clone());
        }
    }
    std::fs::write(dir.join(format!("{id}.json")), base_manifest.to_string()).unwrap();
}

fn tagged_node(node_id: i64, cline_message_index: Option<i64>) -> MessageNode {
    MessageNode {
        node_id,
        parent_node_id: None,
        role: Role::User,
        content: String::new(),
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
        created_at: 0.0,
        metadata: match cline_message_index {
            Some(i) => json!({ "clineMessageIndex": i }),
            None => Value::Null,
        },
    }
}

#[tokio::test]
async fn list_summarizes_manifests_skipping_non_sessions_and_unparseable_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    write_cline_session(
        data_dir,
        "s1",
        &json!({ "metadata": { "title": "  Titled session  " } }),
        &json!({}),
    );
    write_cline_session(
        data_dir,
        "s2",
        &json!({ "prompt": "<task>do <b>things</b></task>" }),
        &json!({}),
    );
    // a subagent id pattern lands its lineage on the summary
    write_cline_session(data_dir, "s1__agent_9", &json!({}), &json!({}));
    // a dir whose only json is the messages file → not a manifest
    std::fs::create_dir_all(data_dir.join("sessions/msg-only")).unwrap();
    std::fs::write(data_dir.join("sessions/msg-only/m.messages.json"), "{}").unwrap();
    // an unparseable manifest is skipped, not fatal
    write_cline_session(data_dir, "broken", &json!({}), &json!({}));
    std::fs::write(data_dir.join("sessions/broken/broken.json"), "{not json").unwrap();
    // a stray file at the sessions root is ignored
    std::fs::write(data_dir.join("sessions/loose.txt"), "x").unwrap();

    let repo = ClineStore::new(data_dir.to_path_buf());
    let sessions = repo.list().await.unwrap();
    let by_id: std::collections::HashMap<&str, &Session> =
        sessions.iter().map(|s| (s.id.as_str(), s)).collect();

    assert_eq!(by_id["s1"].title, "Titled session");
    assert_eq!(by_id["s1"].model, "glm-5-2"); // cline-pass/ prefix stripped
    assert_eq!(by_id["s1"].backend_type, "cline");
    assert_eq!(
        by_id["s1"].created_at,
        (time::OffsetDateTime::parse(
            "2026-01-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339
        )
        .unwrap()
        .unix_timestamp()) as f64
    );
    assert_eq!(by_id["s2"].title, "do things");
    assert_eq!(
        by_id["s1__agent_9"].parent_session_id.as_deref(),
        Some("s1")
    );
    assert_eq!(by_id["s1__agent_9"].agent_id.as_deref(), Some("agent_9"));
    assert!(!by_id.contains_key("msg-only"));
    assert!(!by_id.contains_key("broken"));
}

#[tokio::test]
async fn list_treats_a_missing_sessions_dir_as_empty_and_a_file_root_as_an_error() {
    let missing = tempfile::tempdir().unwrap();
    let repo = ClineStore::new(missing.path().join("nope"));
    assert_eq!(repo.list().await.unwrap(), Vec::<Session>::new());
    assert!(!repo.has_session("x").await.unwrap());

    // `sessions` exists but is a file — exists() is true, read_dir fails
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::write(data_dir.path().join("sessions"), "not a dir").unwrap();
    let repo = ClineStore::new(data_dir.path().to_path_buf());
    let err = repo.list().await.unwrap_err();
    assert!(err.message.contains("Failed to list cline sessions"));
}

#[tokio::test]
async fn list_caches_manifest_reads_until_a_manifest_or_the_dir_set_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    write_cline_session(
        data_dir,
        "s1",
        &json!({ "metadata": { "title": "One" } }),
        &json!({}),
    );
    // a dangling-symlink entry at the sessions root — unstatable, skipped
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        data_dir.join("sessions/nonexistent"),
        data_dir.join("sessions/ghost"),
    )
    .unwrap();
    let repo = ClineStore::new(data_dir.to_path_buf());

    let first = repo.list().await.unwrap();
    assert_eq!(first[0].title, "One");
    assert_eq!(
        first.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["s1"]
    );
    // unchanged stamp → the cached array comes back verbatim
    let second = repo.list().await.unwrap();
    assert_eq!(second, first);

    // editing the manifest bumps its mtime → re-read
    let manifest = data_dir.join("sessions/s1/s1.json");
    std::fs::write(
        &manifest,
        json!({ "session_id": "s1", "cwd": "/work", "metadata": { "title": "Two" } }).to_string(),
    )
    .unwrap();
    let third = repo.list().await.unwrap();
    assert_eq!(third[0].title, "Two");

    // a new session dir busts via the listing stamp
    write_cline_session(data_dir, "s2", &json!({}), &json!({}));
    let mut ids: Vec<String> = repo
        .list()
        .await
        .unwrap()
        .iter()
        .map(|s| s.id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, ["s1", "s2"]);
}

#[tokio::test]
async fn get_by_id_loads_the_transcript_and_degrades_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    write_cline_session(data_dir, "s1", &json!({}), &json!({}));
    // a dir with a manifest but no transcript — from_directory fails reading it
    let dir = data_dir.join("sessions/no-messages");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("no-messages.json"),
        json!({ "session_id": "no-messages", "cwd": "/w" }).to_string(),
    )
    .unwrap();

    let repo = ClineStore::new(data_dir.to_path_buf());
    let found = repo.get_by_id("s1", None).await.unwrap().unwrap();
    assert!(!found.nodes.is_empty());
    assert_eq!(found.backend_type, "cline");

    assert!(repo.get_by_id("ghost", None).await.unwrap().is_none());
    assert!(repo.get_by_id("no-messages", None).await.is_err());
}

#[tokio::test]
async fn has_session_reflects_the_sessions_dir_and_writes_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    write_cline_session(data_dir, "s1", &json!({}), &json!({}));
    let repo = ClineStore::new(data_dir.to_path_buf());
    assert!(repo.has_session("s1").await.unwrap());
    assert!(!repo.has_session("nope").await.unwrap());

    // Writes (SessionWrite): save installs the pair + index row, and the
    // embedded IR makes the read faithful rather than a re-parse.
    let session = sepia_testkit::contract::session("w1", "Written", 1.0);
    repo.save(&session).await.unwrap();
    assert!(repo.has_session("w1").await.unwrap());
    let got = repo.get_by_id("w1", None).await.unwrap().unwrap();
    assert_eq!(got.title, "Written");
    assert_eq!(got.nodes.len(), 1);
    assert_eq!(got.nodes[0].content, "hello from w1");

    // the index row registers the session for `cline --id` resume
    let db = data_dir.join("db/sessions.db");
    assert!(db.exists());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let (status, provider): (String, String) = conn
        .query_row(
            "select status, provider from sessions where session_id = 'w1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(provider, "cline-pass");

    repo.delete("w1").await.unwrap();
    assert!(!repo.has_session("w1").await.unwrap());
    // the native-written session (no sidecar) still reads through the parser
    assert!(repo.get_by_id("s1", None).await.unwrap().is_some());
}

#[tokio::test]
async fn save_refuses_to_overwrite_a_live_owned_session() {
    if !Path::new("/proc").is_dir() {
        return; // pid-aliveness is /proc-based on this platform
    }
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    // a live-looking index row for the session about to be saved
    std::fs::create_dir_all(data_dir.join("db")).unwrap();
    let db = data_dir.join("db/sessions.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(sepia_driver_cline::cline_index::SESSIONS_DDL)
        .unwrap();
    conn.execute(
        "insert into sessions (session_id, source, pid, started_at, status, interactive, provider, model, cwd, workspace_root, enable_tools, enable_spawn, enable_teams, hook_path, updated_at)
         values ('live', 'cli', ?, 'x', 'running', 1, 'cline-pass', 'm', '/w', '/w', 1, 1, 1, '', 'x')",
        [i64::from(std::process::id())],
    )
    .unwrap();
    drop(conn);

    let repo = ClineStore::new(data_dir.to_path_buf());
    let err = repo
        .save(&sepia_testkit::contract::session("live", "L", 1.0))
        .await
        .unwrap_err();
    assert!(err.message.contains("live owner"));
}

/* ---- truncate_session refusals ---------------------------------------- */

fn trunc_err(result: Result<i64, StorageError>) -> String {
    result.unwrap_err().message
}

#[tokio::test]
async fn truncate_refuses_a_dir_with_no_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions/s1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s1.messages.json"), "{}").unwrap();
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("No session metadata json"));
}

#[tokio::test]
async fn truncate_refuses_a_manifest_that_is_not_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions/s1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s1.json"), "{not json").unwrap();
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("not valid JSON"));
}

#[tokio::test]
async fn truncate_refuses_an_unparseable_or_message_less_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions/s1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("s1.json"),
        json!({ "session_id": "s1" }).to_string(),
    )
    .unwrap();
    let repo = ClineStore::new(tmp.path().to_path_buf());
    std::fs::write(dir.join("s1.messages.json"), "{oops").unwrap();
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("not valid JSON"));

    std::fs::write(
        dir.join("s1.messages.json"),
        json!({ "notMessages": true }).to_string(),
    )
    .unwrap();
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("no message array"));
}

#[tokio::test]
async fn truncate_refuses_when_it_cannot_prove_the_transcript_is_the_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions/s1");
    std::fs::create_dir_all(&dir).unwrap();
    // no sessionId on the data, and the file is not named `<id>.messages.json`
    let other = dir.join("shared.messages.json");
    std::fs::write(&other, json!({ "messages": [] }).to_string()).unwrap();
    std::fs::write(
        dir.join("s1.json"),
        json!({ "session_id": "s1", "messages_path": other.to_string_lossy() }).to_string(),
    )
    .unwrap();
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("Cannot prove"));
}

#[tokio::test]
async fn truncate_refuses_a_messages_path_escaping_the_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("shared.messages.json");
    std::fs::write(&file, json!({ "messages": [] }).to_string()).unwrap();
    let dir = tmp.path().join("sessions/s1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("s1.json"),
        json!({ "session_id": "s1", "messages_path": file.to_string_lossy() }).to_string(),
    )
    .unwrap();
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let msg = trunc_err(repo.truncate_session("s1", &[], &[]));
    assert!(msg.contains("escapes the data dir"));
}

#[tokio::test]
async fn truncate_refuses_a_removed_node_with_no_transcript_index() {
    let tmp = tempfile::tempdir().unwrap();
    write_cline_session(tmp.path(), "s1", &json!({}), &json!({}));
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let msg = trunc_err(repo.truncate_session("s1", &[], &[tagged_node(7, None)]));
    assert!(msg.contains("no recorded transcript entry"));
}

#[tokio::test]
async fn truncate_is_a_no_op_when_the_kept_set_already_reaches_the_tail() {
    let tmp = tempfile::tempdir().unwrap();
    write_cline_session(tmp.path(), "s1", &json!({}), &json!({}));
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let removed = repo
        .truncate_session("s1", &[tagged_node(0, Some(99))], &[])
        .unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn truncate_cuts_the_transcript_on_whole_message_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    write_cline_session(tmp.path(), "s1", &json!({}), &json!({}));
    let repo = ClineStore::new(tmp.path().to_path_buf());
    let session = repo.get_by_id("s1", None).await.unwrap().unwrap();
    // Node layout: [system, system, user(idx 0), system-skills, assistant
    // twin ×2 (idx 1)]. Untagged head nodes may be kept without an index;
    // every removed node must carry one — the assistant twins share the
    // same source message and drop together.
    let kept: Vec<MessageNode> = session.nodes[..4].to_vec();
    let removed: Vec<MessageNode> = session.nodes[4..].to_vec();
    assert!(
        removed
            .iter()
            .all(|n| { n.metadata.get("clineMessageIndex").and_then(Value::as_i64) == Some(1) })
    );
    let removed_count = repo.truncate_session("s1", &kept, &removed).unwrap();
    assert_eq!(removed_count, 1);
    let data: Value = serde_json::from_str(
        &std::fs::read_to_string(tmp.path().join("sessions/s1/s1.messages.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(data["messages"].as_array().unwrap().len(), 1);
    assert!(data["updated_at"].as_str().is_some());
}

#[tokio::test]
async fn list_skips_a_session_dir_whose_contents_or_manifest_are_unreadable() {
    if std::env::var("USER").as_deref() == Ok("root") {
        return; // root ignores chmod 0o000
    }
    let tmp = tempfile::tempdir().unwrap();
    let locked = tmp.path().join("sessions/locked");
    std::fs::create_dir_all(&locked).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let half = tmp.path().join("sessions/half");
        std::fs::create_dir_all(&half).unwrap();
        let manifest = half.join("half.json");
        std::fs::write(&manifest, json!({ "session_id": "half" }).to_string()).unwrap();
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o000)).unwrap();

        let repo = ClineStore::new(tmp.path().to_path_buf());
        let listed = repo.list().await.unwrap();
        assert_eq!(listed, Vec::<Session>::new());

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}
