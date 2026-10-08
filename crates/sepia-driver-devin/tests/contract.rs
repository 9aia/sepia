#![allow(clippy::unwrap_used, clippy::pedantic)]

//! DevinStore behavioral contract + golden fixture comparisons.

use std::path::PathBuf;

use sepia_core::storage::SessionRepository;
use sepia_driver_devin::DevinStore;
use sepia_testkit::{assert_json_eq, assert_session_repository_contract};

fn scratch_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sessions.db");
    (dir, db)
}

#[tokio::test]
async fn session_repository_contract() {
    let (_dir, db) = scratch_db();
    let store = DevinStore::open(&db, false).unwrap();
    assert_session_repository_contract(&store).await;
}

#[tokio::test]
async fn readonly_store_refuses_writes() {
    let (_dir, db) = scratch_db();
    {
        let store = DevinStore::open(&db, false).unwrap();
        store
            .save(&sepia_testkit::contract::session("ro-1", "RO1", 1.0))
            .await
            .unwrap();
    }
    let ro = DevinStore::open(&db, true).unwrap();
    let err = ro
        .save(&sepia_testkit::contract::session("ro-2", "RO2", 1.0))
        .await
        .unwrap_err();
    assert!(err.message.contains("read-only"));
    assert!(ro.get_by_id("ro-1", None).await.unwrap().is_some());
}

/// Golden fixtures — each `fixtures/devin/<case>/` gets `store.sql`
/// applied then its `list.json`/`export.*.json` compared against live
/// reads.
#[tokio::test]
async fn golden_fixtures() {
    let base = sepia_testkit::fixture_dir("devin", "");
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
        let store_path = sepia_testkit::materialize_store(&case_dir, scratch.path()).unwrap();
        let store = DevinStore::open(&store_path, true).unwrap();

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
