#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_meta::{MetaStore, RunSpan, SessionMeta, append_span};
use serde_json::json;

fn store() -> (MetaStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = MetaStore::open(&dir.path().join("meta.json"));
    (store, dir)
}

#[test]
fn patch_persists_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let store = MetaStore::open(&path);
    store.patch(
        "s1",
        &SessionMeta {
            pinned: Some(true),
            project_ids: Some(vec!["p1".into()]),
            ..SessionMeta::default()
        },
    );
    let reloaded = MetaStore::open(&path);
    let meta = reloaded.of("s1").unwrap();
    assert_eq!(meta.pinned, Some(true));
    assert_eq!(meta.project_ids, Some(vec!["p1".to_string()]));
}

#[test]
fn patch_merges_fields() {
    let (store, _dir) = store();
    store.patch(
        "s1",
        &SessionMeta {
            title: Some("t".into()),
            ..SessionMeta::default()
        },
    );
    store.patch(
        "s1",
        &SessionMeta {
            archived: Some(true),
            ..SessionMeta::default()
        },
    );
    let meta = store.of("s1").unwrap();
    assert_eq!(meta.title.as_deref(), Some("t"));
    assert_eq!(meta.archived, Some(true));
}

#[test]
fn spans_dedupe_consecutive_agent_node() {
    let span = |at: f64| RunSpan {
        at,
        agent: "devin".into(),
        node: "n1".into(),
    };
    assert_eq!(append_span(&[], span(1.0)).len(), 1);
    let once = append_span(&[span(1.0)], span(2.0));
    assert_eq!(once.len(), 1); // same agent+node — same run continuing
    let other = append_span(
        &[span(1.0)],
        RunSpan {
            at: 2.0,
            agent: "cline".into(),
            node: "n1".into(),
        },
    );
    assert_eq!(other.len(), 2);
}

#[test]
fn projects_lifecycle() {
    let (store, _dir) = store();
    let p = store.create_project("alpha");
    assert!(p.id.starts_with("proj_"));
    assert!(store.rename_project(&p.id, "beta"));
    assert_eq!(store.list_projects()[0].name, "beta");
    // Sessions assigned the project lose the id on delete.
    store.patch(
        "s1",
        &SessionMeta {
            project_ids: Some(vec![p.id.clone()]),
            ..SessionMeta::default()
        },
    );
    store.delete_project(&p.id);
    assert!(store.of("s1").unwrap().project_ids.unwrap().is_empty());
    assert!(!store.rename_project(&p.id, "x"));
}

#[test]
fn corrupt_file_degrades_to_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    std::fs::write(&path, "{not json{").unwrap();
    let store = MetaStore::open(&path);
    assert!(store.sessions().is_empty());
}

#[test]
fn v1_flat_file_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    std::fs::write(
        &path,
        json!({
            "s1": { "title": "old", "projectId": "p9", "pinned": true }
        })
        .to_string(),
    )
    .unwrap();
    let store = MetaStore::open(&path);
    let meta = store.of("s1").unwrap();
    assert_eq!(meta.title.as_deref(), Some("old"));
    assert_eq!(meta.project_ids, Some(vec!["p9".to_string()]));
}

#[test]
fn ensure_project_is_idempotent() {
    let (store, _dir) = store();
    store.ensure_project("p1", "one");
    let p = store.ensure_project("p1", "two"); // refreshes the name
    assert_eq!(p.name, "two");
    assert_eq!(store.list_projects().len(), 1);
}

#[test]
fn config_roundtrip() {
    let (store, _dir) = store();
    store.set_config("sidebar", json!({"collapsed": true}));
    assert_eq!(store.config()["sidebar"], json!({"collapsed": true}));
}
