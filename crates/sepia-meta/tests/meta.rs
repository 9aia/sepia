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
fn legacy_projects_and_config_keys_are_dropped_on_flush() {
    // P-006 phase 3: meta.json v2 carried `projects`/`config` sections;
    // the store is sessions-only now — the keys are ignored on read and
    // gone from the file after the next mutation's flush.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "sessions": {"s1": {"title": "T"}},
            "projects": {"p1": {"name": "Web"}},
            "config": {"theme": "dark"},
        })
        .to_string(),
    )
    .unwrap();
    let store = MetaStore::open(&path);
    assert_eq!(store.of("s1").unwrap().title.as_deref(), Some("T"));
    store.patch(
        "s2",
        &SessionMeta {
            pinned: Some(true),
            ..SessionMeta::default()
        },
    );
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(written.get("projects").is_none());
    assert!(written.get("config").is_none());
    assert!(written["sessions"]["s2"]["pinned"].as_bool().unwrap());
}

#[test]
fn spans_model_and_patch_edges() {
    use sepia_meta::{RunSpan, SessionMeta};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    // Crafted meta file exercising normalize_session tails: model as a
    // non-string, spans with invalid entries filtered out, projectIds in
    // array and legacy singular shapes plus a dropped number. A legacy
    // `config` section rides along — ignored since phase 3.
    std::fs::write(
        &path,
        serde_json::json!({
            "sessions": {
                "s1": {
                    "title": "T",
                    "model": 42,
                    "spans": [
                        {"agent":"devin","node":"n1","at":1.0},
                        {"agent":""},
                        7
                    ],
                    "projectIds": ["proj-a", 42, {"name":"x"}],
                    "projectId": "ignored-when-array"
                },
                "s2": { "projectId": "proj-solo", "pinned": "yes" }
            },
            "config": {"historyLimit": 10}
        })
        .to_string(),
    )
    .unwrap();
    let store = MetaStore::open(&path);
    let meta = store.of("s1").unwrap();
    assert_eq!(meta.model, None);
    assert_eq!(meta.spans.as_ref().map(Vec::len), Some(1));
    let projects = meta.project_ids.unwrap_or_default();
    assert_eq!(projects, vec!["proj-a"]);
    let solo = store.of("s2").unwrap();
    assert_eq!(solo.project_ids.unwrap_or_default(), vec!["proj-solo"]);
    assert!(solo.pinned.is_none());

    // patch with spans + model set.
    let store2 = MetaStore::open(&dir.path().join("m2.json"));
    store2.patch(
        "s2",
        &SessionMeta {
            spans: Some(vec![RunSpan {
                agent: "devin".into(),
                node: "n".into(),
                at: 1.0,
            }]),
            model: Some(Some("claude".into())),
            ..SessionMeta::default()
        },
    );
    let meta = store2.of("s2").unwrap();
    assert_eq!(meta.model, Some(Some("claude".to_string())));

    // Corrupt + non-object + non-json files degrade to empty.
    std::fs::write(dir.path().join("bad.json"), "{not json").unwrap();
    let s = MetaStore::open(&dir.path().join("bad.json"));
    assert!(s.sessions().is_empty());
    std::fs::write(dir.path().join("arr.json"), "[1,2]").unwrap();
    let s = MetaStore::open(&dir.path().join("arr.json"));
    assert!(s.sessions().is_empty());
}
