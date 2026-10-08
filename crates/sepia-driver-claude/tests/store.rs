#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Port of `packages/claude/tests/ClaudeCodeRepository.test.ts` — the
//! filesystem-scanning repository plus `truncateClaudeTranscript`.

use std::path::{Path, PathBuf};

use sepia_core::domain::{MessageNode, Role, Session};
use sepia_core::rewind::{self, RewindTarget};
use sepia_core::storage::SessionRepository;
use sepia_driver_claude::{ClaudeStore, from_file};
use serde_json::{Value, json};

fn line(entry: Value) -> String {
    serde_json::to_string(&entry).unwrap()
}

fn user_entry(uuid: &str, parent: Value, content: Value, extra: Value) -> Value {
    let mut entry = json!({
        "type": "user",
        "uuid": uuid,
        "parentUuid": parent,
        "sessionId": "sess-1",
        "isSidechain": false,
        "cwd": "/work/proj",
        "gitBranch": "main",
        "version": "2.1.0",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "message": { "role": "user", "content": content },
    });
    if let Value::Object(extra) = extra {
        entry.as_object_mut().unwrap().extend(extra);
    }
    entry
}

/// Write `files` (relative path → content) under a fresh temp dir; returns
/// (TempDir, projectsDir equivalent = the root itself, matching the TS
/// `makeStore` convention where the root plays the projects dir).
fn make_store(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let file_path = root.path().join(rel);
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, content).unwrap();
    }
    let path = root.path().to_path_buf();
    (root, path)
}

fn main_transcript() -> String {
    [
        line(json!({ "type": "summary", "summary": "Main session", "leafUuid": "u1" })),
        line(user_entry(
            "u1",
            Value::Null,
            json!("main prompt"),
            json!({}),
        )),
    ]
    .join("\n")
}

fn subagent_transcript() -> String {
    line(json!({
        "type": "user", "uuid": "a1", "parentUuid": null,
        "sessionId": "sess-1", "agentId": "agent-9", "isSidechain": true,
        "cwd": "/work/proj", "timestamp": "2026-01-01T00:00:05.000Z",
        "message": { "role": "user", "content": "sub task" },
    }))
}

fn transcript(id: &str, cwd: &str) -> String {
    format!(
        "{}\n",
        [
            line(json!({ "type": "summary", "summary": "T", "leafUuid": "u1" })),
            line(json!({
                "type": "user", "uuid": "u1", "parentUuid": null, "sessionId": id,
                "cwd": cwd, "timestamp": "2026-01-01T00:00:01.000Z",
                "message": { "role": "user", "content": "prompt one" },
            })),
            line(json!({
                "type": "assistant", "uuid": "u2", "parentUuid": "u1", "sessionId": id,
                "timestamp": "2026-01-01T00:00:02.000Z",
                "message": { "role": "assistant", "content": [{ "type": "text", "text": "answer one" }] },
            })),
        ]
        .join("\n")
    )
}

fn write_transcript(projects_dir: &Path, slug: &str, id: &str, cwd: &str) -> PathBuf {
    let dir = projects_dir.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join(format!("{id}.jsonl"));
    std::fs::write(&file_path, transcript(id, cwd)).unwrap();
    file_path
}

fn session_fixture(id: &str, cwd: &str, parent: Option<&str>) -> Session {
    let node = |node_id: i64, parent_node_id: Option<i64>, role: Role, content: &str| MessageNode {
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
        created_at: 1_700_000_000.0 + node_id as f64 * 50.0,
        metadata: Value::Null,
    };
    Session {
        id: id.into(),
        title: "t".into(),
        working_directory: cwd.into(),
        backend_type: "claude".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_100.0,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: parent.map(str::to_string),
        agent_id: None,
        checkpoints: Vec::new(),
        metadata: Value::Null,
        nodes: vec![
            node(0, None, Role::User, "prompt"),
            node(1, Some(0), Role::Assistant, "answer"),
        ],
        prompt_history: Vec::new(),
    }
}

#[tokio::test]
async fn lists_main_legacy_and_subagents_transcripts() {
    let (_root, projects_dir) = make_store(&[
        ("-work-proj/sess-1.jsonl", &main_transcript()),
        // legacy layout: agent-*.jsonl sits beside the parent's file
        ("-work-proj/agent-9.jsonl", &subagent_transcript()),
        // current layout: <parent-uuid>/subagents/agent-*.jsonl
        (
            "-work-proj/sess-2/subagents/agent-7.jsonl",
            &subagent_transcript(),
        ),
        ("-work-proj/sess-2/subagents/notes.txt", "not a transcript"),
        ("-work-proj/empty.jsonl", ""),
        ("-work-proj/notes.txt", "ignored"),
        ("-work-proj/attic/readme.md", "not a transcript dir"),
    ]);
    let repo = ClaudeStore::new(projects_dir);
    let sessions = repo.list().await.unwrap();
    let by_id: std::collections::HashMap<&str, &Session> =
        sessions.iter().map(|s| (s.id.as_str(), s)).collect();

    assert_eq!(by_id["sess-1"].title, "Main session");
    assert_eq!(by_id["sess-1"].working_directory, "/work/proj");
    // legacy subagent links to its parent through the entries' sessionId
    assert_eq!(
        by_id["agent-9"].parent_session_id.as_deref(),
        Some("sess-1")
    );
    // new-layout subagent also carries the parent uuid from its directory
    // name — the entries' sessionId wins when it disagrees
    assert_eq!(
        by_id["agent-7"].parent_session_id.as_deref(),
        Some("sess-1")
    );
    assert_eq!(by_id["agent-7"].agent_id.as_deref(), Some("agent-9"));
    assert!(!by_id.contains_key("empty"));
    // summaries carry no nodes
    assert!(by_id["sess-1"].nodes.is_empty());
}

#[tokio::test]
async fn treats_a_missing_projects_dir_as_empty() {
    let root = tempfile::tempdir().unwrap();
    let repo = ClaudeStore::new(root.path().join("does-not-exist"));
    assert!(repo.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn get_by_id_parses_the_full_transcript_and_unknown_ids_are_none() {
    let (_root, projects_dir) = make_store(&[("-work-proj/sess-1.jsonl", &main_transcript())]);
    let repo = ClaudeStore::new(projects_dir);

    let session = repo.get_by_id("sess-1", None).await.unwrap().unwrap();
    assert!(!session.nodes.is_empty());
    assert_eq!(session.nodes[0].content, "main prompt");

    assert!(repo.get_by_id("ghost", None).await.unwrap().is_none());
}

#[tokio::test]
async fn has_session_reflects_the_files_on_disk() {
    let (_root, projects_dir) = make_store(&[("-work-proj/sess-1.jsonl", &main_transcript())]);
    let repo = ClaudeStore::new(projects_dir);
    assert!(repo.has_session("sess-1").await.unwrap());
    assert!(!repo.has_session("nope").await.unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn skips_unstatable_unreadable_and_non_dir_entries() {
    use std::os::unix::fs::symlink;

    let (_root, projects_dir) = make_store(&[
        ("-work-proj/sess-1.jsonl", &main_transcript()),
        // a `subagents` path that is a file, not a directory
        ("-work-proj/sess-3/subagents", "not a dir"),
        // a `.jsonl` that is actually a directory — unreadable as a file
        ("-work-proj/dir.jsonl/x", "x"),
        // a stray non-dir entry at the projects root
        ("stray.txt", "loose file"),
    ]);
    // a project-dir entry whose stat fails (dangling symlink)
    symlink("ghost-target", projects_dir.join("-broken")).unwrap();

    let repo = ClaudeStore::new(projects_dir);
    let sessions = repo.list().await.unwrap();
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["sess-1"]);

    // the .jsonl "file" that is a directory is skipped on list and a
    // StorageError on get_by_id (EISDIR is not a not-found)
    let err = repo.get_by_id("dir", None).await.unwrap_err();
    assert!(err.message.contains("Failed to read claude session"));
}

#[cfg(unix)]
#[tokio::test]
async fn tolerates_an_unreadable_project_dir_and_a_vanished_transcript() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    let (_root, projects_dir) = make_store(&[("-locked/sess-1.jsonl", &main_transcript())]);
    std::fs::set_permissions(
        projects_dir.join("-locked"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let repo = ClaudeStore::new(projects_dir.clone());
    // inner readDirectory failure degrades that project to empty
    assert!(repo.list().await.unwrap().is_empty());

    // the file shows up in the scan but is gone/unreadable at read time:
    // not-found errors map to none
    std::fs::set_permissions(
        projects_dir.join("-locked"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::remove_file(projects_dir.join("-locked/sess-1.jsonl")).unwrap();
    symlink("ghost", projects_dir.join("-locked/sess-1.jsonl")).unwrap();
    assert!(repo.get_by_id("sess-1", None).await.unwrap().is_none());
}

#[tokio::test]
async fn list_and_has_session_fail_when_the_root_cannot_be_listed() {
    // a projectsDir that is a file: exists() is true, readDirectory throws
    let root = tempfile::tempdir().unwrap();
    let not_a_dir = root.path().join("projects");
    std::fs::write(&not_a_dir, "nope").unwrap();
    let repo = ClaudeStore::new(not_a_dir);
    let err = repo.list().await.unwrap_err();
    assert!(err.message.contains("Failed to list claude sessions"));
    let err = repo.has_session("x").await.unwrap_err();
    assert!(err.message.contains("Failed to check claude session"));
}

#[tokio::test]
async fn an_empty_or_missing_projects_root_lists_nothing() {
    let root = tempfile::tempdir().unwrap();
    let repo = ClaudeStore::new(root.path().join("absent"));
    assert!(repo.list().await.unwrap().is_empty());
    assert!(!repo.has_session("x").await.unwrap());
}

#[tokio::test]
async fn save_writes_the_canonical_layout_and_delete_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let projects_dir = root.path().join("projects");
    let repo = ClaudeStore::new(projects_dir.clone());

    repo.save(&session_fixture("new-1", "/work/proj", None))
        .await
        .unwrap();
    let file_path = projects_dir.join("-work-proj/new-1.jsonl");
    assert!(!std::fs::read_to_string(&file_path).unwrap().is_empty());
    assert!(repo.get_by_id("new-1", None).await.unwrap().is_some());

    // subagent lands under the parent's subagents dir
    repo.save(&session_fixture("agent-7", "/work/proj", Some("new-1")))
        .await
        .unwrap();
    let sub_path = projects_dir.join("-work-proj/new-1/subagents/agent-7.jsonl");
    assert!(!std::fs::read_to_string(&sub_path).unwrap().is_empty());

    repo.delete("new-1").await.unwrap();
    assert!(repo.get_by_id("new-1", None).await.unwrap().is_none());
    // deleting again is a no-op
    repo.delete("new-1").await.unwrap();
}

#[tokio::test]
async fn save_places_a_subagent_under_the_project_dir_that_holds_its_parent() {
    // The parent's transcript lives under a slug that does NOT match the
    // subagent's own cwd — save must reuse the holder's dir anyway.
    let (_root, projects_dir) = make_store(&[("-elsewhere/parent-1.jsonl", &main_transcript())]);
    let repo = ClaudeStore::new(projects_dir.clone());
    let mut child = session_fixture("agent-7", "/work/proj", Some("parent-1"));
    child.id = "agent-7".into();
    repo.save(&child).await.unwrap();
    assert!(
        projects_dir
            .join("-elsewhere/parent-1/subagents/agent-7.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn save_refuses_unsafe_ids() {
    let root = tempfile::tempdir().unwrap();
    let repo = ClaudeStore::new(root.path().join("p"));
    let err = repo
        .save(&session_fixture("../escape", "/w", None))
        .await
        .unwrap_err();
    assert!(err.message.contains("not a safe file name"));
}

#[tokio::test]
async fn delete_refuses_unsafe_ids_and_ignores_unknown_ones() {
    let (_root, projects_dir) = make_store(&[("-work-proj/sess-1.jsonl", &main_transcript())]);
    let repo = ClaudeStore::new(projects_dir.clone());
    let err = repo.delete("a/b").await.unwrap_err();
    assert!(err.message.contains("not a safe file name"));
    // unknown id: the scan finds nothing to remove and resolves quietly
    repo.delete("ghost").await.unwrap();
    assert!(projects_dir.join("-work-proj/sess-1.jsonl").exists());
}

#[tokio::test]
async fn save_tolerates_a_projects_root_it_cannot_list() {
    // projectsDir is a file: readDirectory fails and the holder scan degrades
    // to "no known holder" before the (also failing) mkdir surfaces.
    let root = tempfile::tempdir().unwrap();
    let not_a_dir = root.path().join("projects");
    std::fs::write(&not_a_dir, "nope").unwrap();
    let repo = ClaudeStore::new(not_a_dir);
    let child = session_fixture("agent-7", "/w", Some("parent-1"));
    let err = repo.save(&child).await.unwrap_err();
    assert!(err.message.contains("Failed to save claude session"));
}

#[cfg(unix)]
#[tokio::test]
async fn save_tolerates_a_broken_parent_probe() {
    use std::os::unix::fs::symlink;

    // A candidate project holding a symlink loop where the parent transcript
    // would sit: the exists() probe fails (ELOOP) and the scan moves on.
    let (_root, projects_dir) = make_store(&[("-work-proj/sess-1.jsonl", &main_transcript())]);
    std::fs::create_dir_all(projects_dir.join("-other")).unwrap();
    let loop_path = projects_dir.join("-other/parent-9.jsonl");
    symlink(&loop_path, &loop_path).unwrap();
    let repo = ClaudeStore::new(projects_dir.clone());
    let child = session_fixture("agent-9", "/work/proj", Some("parent-9"));
    repo.save(&child).await.unwrap();
    assert!(
        projects_dir
            .join("-work-proj/parent-9/subagents/agent-9.jsonl")
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn delete_survives_an_unresolvable_side_dir_probe() {
    use std::os::unix::fs::symlink;

    let (_root, projects_dir) = make_store(&[("-work-proj/sess-5.jsonl", &main_transcript())]);
    // `<slug>/sess-5` is a symlink loop — the post-delete exists() check fails
    // with ELOOP instead of answering, and delete still completes.
    let loop_path = projects_dir.join("-work-proj/sess-5");
    symlink(&loop_path, &loop_path).unwrap();
    let repo = ClaudeStore::new(projects_dir.clone());
    repo.delete("sess-5").await.unwrap();
    assert!(!projects_dir.join("-work-proj/sess-5.jsonl").exists());
}

#[tokio::test]
async fn list_caches_summaries_until_the_transcript_set_changes() {
    let root = tempfile::tempdir().unwrap();
    let projects_dir = root.path().join("projects");
    let file_path = write_transcript(&projects_dir, "-work-a", "sess-a", "/work/a");
    let repo = ClaudeStore::new(projects_dir.clone());

    let first = repo.list().await.unwrap();
    assert_eq!(
        first.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["sess-a"]
    );
    // unchanged stamp → the cached array comes back verbatim
    let second = repo.list().await.unwrap();
    assert_eq!(first, second);

    // editing a transcript bumps its mtime → re-read
    std::fs::write(
        &file_path,
        transcript("sess-a", "/work/a").replace("\"summary\":\"T\"", "\"summary\":\"Edited\""),
    )
    .unwrap();
    let file = std::fs::File::options()
        .write(true)
        .open(&file_path)
        .unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
    let third = repo.list().await.unwrap();
    assert_eq!(
        third.iter().find(|s| s.id == "sess-a").unwrap().title,
        "Edited"
    );

    // a new transcript busts via the file-set signature
    write_transcript(&projects_dir, "-work-b", "sess-b", "/work/b");
    let mut ids: Vec<String> = repo
        .list()
        .await
        .unwrap()
        .iter()
        .map(|s| s.id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, ["sess-a", "sess-b"]);
}

#[tokio::test]
async fn from_file_resolves_id_cwd_and_subagent_parentage_from_the_path() {
    let (_root, projects_dir) = make_store(&[
        ("-work-proj/sess-1.jsonl", &main_transcript()),
        (
            "-work-proj/sess-1/subagents/agent-9.jsonl",
            &subagent_transcript(),
        ),
    ]);

    let main = from_file(&projects_dir.join("-work-proj/sess-1.jsonl"), None, None).unwrap();
    assert_eq!(main.id, "sess-1");
    assert_eq!(main.working_directory, "/work/proj");

    let sub = from_file(
        &projects_dir.join("-work-proj/sess-1/subagents/agent-9.jsonl"),
        None,
        None,
    )
    .unwrap();
    assert_eq!(sub.id, "agent-9");
    assert_eq!(sub.parent_session_id.as_deref(), Some("sess-1"));

    let err = from_file(&projects_dir.join("-work-proj/missing.jsonl"), None, None).unwrap_err();
    assert!(err.message.contains("Claude Code transcript not found"));
}

/* ---- truncateClaudeTranscript ---- */

#[tokio::test]
async fn truncate_rewrites_the_file_to_end_at_the_cut_with_kept_bytes_intact() {
    let root = tempfile::tempdir().unwrap();
    let projects_dir = root.path().join("projects");
    let file_path = write_transcript(&projects_dir, "-work-proj", "sess-1", "/work/proj");
    // append a second turn to truncate away
    let extra = [
        line(json!({
            "type": "user", "uuid": "u3", "parentUuid": "u2", "sessionId": "sess-1",
            "timestamp": "2026-01-01T00:00:03.000Z",
            "message": { "role": "user", "content": "prompt two" },
        })),
        line(json!({
            "type": "assistant", "uuid": "u4", "parentUuid": "u3", "sessionId": "sess-1",
            "timestamp": "2026-01-01T00:00:04.000Z",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "answer two" }] },
        })),
    ]
    .join("\n");
    let original = std::fs::read_to_string(&file_path).unwrap();
    std::fs::write(&file_path, format!("{original}{extra}\n")).unwrap();

    let repo = ClaudeStore::new(projects_dir.clone());
    let session = repo.get_by_id("sess-1", None).await.unwrap().unwrap();
    let target = session
        .nodes
        .iter()
        .find(|n| n.content == "answer one")
        .unwrap();
    let planned = rewind::plan_rewind(&session, &RewindTarget::NodeId(target.node_id)).unwrap();

    let removed = repo
        .truncate_transcript("sess-1", &planned.kept, &planned.removed)
        .unwrap();
    assert_eq!(removed, 2);
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), original);
}

#[tokio::test]
async fn truncate_fails_for_an_unknown_session() {
    let root = tempfile::tempdir().unwrap();
    let projects_dir = root.path().join("projects");
    std::fs::create_dir_all(&projects_dir).unwrap();
    let repo = ClaudeStore::new(projects_dir);
    assert!(repo.truncate_transcript("ghost", &[], &[]).is_err());
}

#[tokio::test]
async fn truncate_fails_when_a_kept_node_has_no_locatable_entry() {
    let root = tempfile::tempdir().unwrap();
    let projects_dir = root.path().join("projects");
    write_transcript(&projects_dir, "-work-proj", "sess-1", "/work/proj");
    let repo = ClaudeStore::new(projects_dir);
    let orphan = MessageNode {
        node_id: 99,
        parent_node_id: None,
        role: Role::User,
        content: "no uuid metadata".into(),
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
        metadata: Value::Null,
    };
    assert!(repo.truncate_transcript("sess-1", &[orphan], &[]).is_err());
}
