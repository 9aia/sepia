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

#[test]
fn spans_projects_model_and_patch_edges() {
    use sepia_meta::{RunSpan, SessionMeta};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    // Crafted meta file exercising normalize_session tails: model as a
    // non-string, spans with invalid entries filtered out, projects in
    // object and string shapes plus a dropped number.
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

#[test]
fn v2_projects_map_entries_parse_each_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "sessions": {},
            "projects": {
                "p1": {"name": "Object"},
                "p2": "Plain",
                "p3": 42
            },
        })
        .to_string(),
    )
    .unwrap();
    let store = MetaStore::open(&path);
    let names = store
        .list_projects()
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(names.get("p1").map(|p| p.name.as_str()), Some("Object"));
    assert_eq!(names.get("p2").map(|p| p.name.as_str()), Some("Plain"));
    assert!(!names.contains_key("p3"));
}
