#![allow(clippy::unwrap_used, clippy::pedantic)]

//! ClaudeStore behavioral contract + golden fixture comparisons.

use sepia_core::storage::SessionRepository;
use sepia_driver_claude::ClaudeStore;
use sepia_testkit::{assert_json_eq, assert_session_repository_contract};

#[tokio::test]
async fn session_repository_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = ClaudeStore::new(dir.path().join("projects"));
    assert_session_repository_contract(&store).await;
}

/// Golden fixtures — each `fixtures/claude/<case>/` gets its `store/` dir
/// (a `~/.claude` analogue with `projects/` + `file-history/`) copied to a
/// scratch dir, then `list.json`/`export.*.json` compared against live reads
/// over `<scratch>/store/projects`.
#[tokio::test]
async fn golden_fixtures() {
    let base = sepia_testkit::fixture_dir("claude", "");
    let base = base.parent().unwrap().to_path_buf();
    if !base.is_dir() {
        return; // fixtures not generated yet
    }
    for case in std::fs::read_dir(&base).unwrap() {
        let case_dir = case.unwrap().path();
        if !case_dir.is_dir() {
            continue;
        }
        let scratch = tempfile::tempdir().unwrap();
        let Ok(claude_dir) = sepia_testkit::materialize_store(&case_dir, scratch.path()) else {
            continue; // a case without a store/ skeleton is not runnable
        };
        let store = ClaudeStore::new(claude_dir.join("projects"));

        if let Ok(expected) = sepia_testkit::load_expected(&case_dir, "list.json") {
            let listed = store.list().await.unwrap();
            let actual = serde_json::Value::Array(
                listed
                    .iter()
                    .map(|s| sepia_core::wire::session_to_json(s).unwrap())
                    .collect(),
            );
            assert_json_eq(&actual, &expected, &format!("{case_dir:?} list"));
        }
        for entry in std::fs::read_dir(&case_dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            let Some(id) = name
                .strip_prefix("export.")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            let expected = sepia_testkit::load_expected(&case_dir, &name).unwrap();
            let session = store.get_by_id(id, None).await.unwrap().unwrap();
            let actual = sepia_core::wire::session_to_json(&session).unwrap();
            assert_json_eq(&actual, &expected, &format!("{case_dir:?} {name}"));
        }
    }
}
