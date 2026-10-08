#![allow(clippy::unwrap_used, clippy::pedantic)]

//! ClineStore behavioral contract + golden fixture comparisons.

use sepia_core::storage::SessionRepository;
use sepia_driver_cline::ClineStore;
use sepia_testkit::{assert_json_eq, assert_session_repository_contract};

#[tokio::test]
async fn session_repository_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = ClineStore::new(dir.path().to_path_buf());
    assert_session_repository_contract(&store).await;
}

/// Golden fixtures — each `fixtures/cline/<case>/` gets its `store/` tree
/// materialized (a Cline data dir) then `list.json`/`export.*.json`
/// compared against live reads.
#[tokio::test]
async fn golden_fixtures() {
    let base = sepia_testkit::fixture_dir("cline", "");
    if !base.is_dir() {
        return; // fixtures not generated yet
    }
    for case in std::fs::read_dir(&base).unwrap() {
        let case_dir = case.unwrap().path();
        if !case_dir.is_dir() {
            continue;
        }
        let scratch = tempfile::tempdir().unwrap();
        let Ok(store_path) = sepia_testkit::materialize_store(&case_dir, scratch.path()) else {
            continue;
        };
        let store = ClineStore::new(store_path);

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
            // `export.<id>.json` carries fresh `chatcmpl-tool-*` ids minted per
            // read — non-reproducible by design. `export-normalized.<id>.json`
            // maps them to `call-<n>` in first-seen order; do the same here.
            let Some(id) = name
                .strip_prefix("export-normalized.")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            let expected = sepia_testkit::load_expected(&case_dir, &name).unwrap();
            let session = store.get_by_id(id, None).await.unwrap().unwrap();
            let mut actual = sepia_core::wire::session_to_json(&session).unwrap();
            normalize_tool_call_ids(&mut actual);
            assert_json_eq(&actual, &expected, &format!("{case_dir:?} {name}"));
        }
    }
}

/// Rewrites every `chatcmpl-tool-*` string value to `call-<n>`, numbered in
/// first-seen document order — the fixture's `export-normalized` shape.
fn normalize_tool_call_ids(value: &mut serde_json::Value) {
    let mut map = std::collections::HashMap::<String, usize>::new();
    fn walk(v: &mut serde_json::Value, map: &mut std::collections::HashMap<String, usize>) {
        match v {
            serde_json::Value::String(s) if s.starts_with("chatcmpl-tool-") => {
                let next = map.len();
                let n = *map.entry(s.clone()).or_insert(next);
                *s = format!("call-{n}");
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, map);
                }
            }
            serde_json::Value::Object(map_obj) => {
                for (_, v) in map_obj.iter_mut() {
                    walk(v, map);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut map);
}
