#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Testkit's own tests — the json_eq diff printer, fixture
//! materialization (store dirs, mtimes, store.sql), and the binary
//! locator's search order.

use sepia_testkit::{assert_json_eq, fixture_dir, json_eq, load_expected, materialize_store};
use serde_json::{Value, json};

// ---------- json_eq --------------------------------------------------------------

#[test]
fn json_eq_semantics() {
    assert!(json_eq(&json!(1), &json!(1.0))); // numeric tolerance
    assert!(!json_eq(&json!(1), &json!(2.0)));
    assert!(json_eq(
        &json!({"a": [1, {"b": null}]}),
        &json!({"a": [1.0, {"b": null}]})
    ));
    assert!(!json_eq(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    assert!(!json_eq(&json!([1, 2]), &json!([1, 2, 3])));
    assert!(!json_eq(&json!({"a": 1}), &json!({"a": "1"})));
    assert!(!json_eq(&json!([1]), &json!({"0": 1})));
    assert!(json_eq(&json!("s"), &json!("s")));
}

#[test]
fn assert_json_eq_passes_quietly() {
    assert_json_eq(&json!({"n": 1}), &json!({"n": 1.0}), "ctx");
}

#[test]
#[should_panic(expected = "JSON mismatch in ctx")]
fn assert_json_eq_missing_and_extra_keys() {
    assert_json_eq(
        &json!({"have": 1, "extra": true}),
        &json!({"have": 1, "want": 2}),
        "ctx",
    );
}

#[test]
#[should_panic(expected = "length differs")]
fn assert_json_eq_array_length_differs() {
    assert_json_eq(&json!([1, 2, 3]), &json!([1]), "ctx");
}

#[test]
#[should_panic(expected = "value differs")]
fn assert_json_eq_leaf_differs() {
    assert_json_eq(&json!({"k": "a"}), &json!({"k": "b"}), "ctx");
}

#[test]
#[should_panic(expected = "truncated")]
fn assert_json_eq_huge_values_truncate_the_report() {
    // The panic report truncates values past 4000 chars.
    let big = "x".repeat(9000);
    assert_json_eq(&json!({"k": big}), &json!({"k": "y"}), "ctx");
}

#[test]
#[should_panic(expected = "missing key")]
fn assert_json_eq_nested_diff_walks_children() {
    let a = json!({"o": {"deep": [{"x": 1}]}});
    let b = json!({"o": {"deep": [{"x": 2, "y": 3}]}});
    assert_json_eq(&a, &b, "ctx");
}

// ---------- fixtures ---------------------------------------------------------------

#[test]
fn fixture_dir_resolves_under_the_manifest() {
    let dir = fixture_dir("devin", "case1");
    assert!(dir.ends_with("fixtures/devin/case1"));
}

#[test]
fn load_expected_reads_json_and_surfaces_errors() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("ok.json"), "{\"a\": 1}").unwrap();
    assert_eq!(load_expected(tmp.path(), "ok.json").unwrap()["a"], 1);
    assert!(load_expected(tmp.path(), "missing.json").is_err());
    std::fs::write(tmp.path().join("bad.json"), "nope").unwrap();
    assert!(load_expected(tmp.path(), "bad.json").is_err());
}

#[test]
fn materialize_store_copies_a_store_dir_and_applies_mtimes() {
    let tmp = tempfile::tempdir().unwrap();
    let case = tmp.path().join("case");
    let store = case.join("store/nested");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("a.txt"), "body").unwrap();
    std::fs::write(case.join("mtimes.txt"), "nested/a.txt\t1234.5\n").unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();

    let out = materialize_store(&case, &scratch).unwrap();
    assert_eq!(out, scratch.join("store"));
    assert_eq!(
        std::fs::read_to_string(out.join("nested/a.txt")).unwrap(),
        "body"
    );
    let mtime = std::fs::metadata(out.join("nested/a.txt"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    assert!((mtime.as_secs_f64() - 1234.5).abs() < 0.01);
}

#[test]
fn materialize_store_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let case = tmp.path().join("case");
    std::fs::create_dir_all(&case).unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();

    // Neither store/ nor store.sql → NotFound.
    assert!(materialize_store(&case, &scratch).is_err());

    // A bad mtimes manifest line → InvalidData.
    let store = case.join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("a.txt"), "b").unwrap();
    std::fs::write(case.join("mtimes.txt"), "no-tab-here\n").unwrap();
    assert!(materialize_store(&case, &scratch).is_err());
    std::fs::write(case.join("mtimes.txt"), "a.txt\tnotafloat\n").unwrap();
    assert!(materialize_store(&case, &scratch).is_err());
    // An mtime for a file that isn't there → error surfaces.
    std::fs::write(case.join("mtimes.txt"), "ghost.txt\t1\n").unwrap();
    assert!(materialize_store(&case, &scratch).is_err());

    // store.sql applies into a fresh db.
    let case2 = tmp.path().join("case2");
    std::fs::create_dir_all(&case2).unwrap();
    std::fs::write(
        case2.join("store.sql"),
        "CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);",
    )
    .unwrap();
    let scratch2 = tmp.path().join("scratch2");
    std::fs::create_dir(&scratch2).unwrap();
    let db = materialize_store(&case2, &scratch2).unwrap();
    assert_eq!(db, scratch2.join("store.db"));
    assert!(db.exists());

    // Broken sql → error.
    let case3 = tmp.path().join("case3");
    std::fs::create_dir_all(&case3).unwrap();
    std::fs::write(case3.join("store.sql"), "THIS IS NOT SQL;").unwrap();
    let scratch3 = tmp.path().join("scratch3");
    std::fs::create_dir(&scratch3).unwrap();
    assert!(materialize_store(&case3, &scratch3).is_err());
}

// ---------- the conformance suite gets a sanity drive -------------------------------

#[tokio::test]
async fn contract_suite_passes_against_a_memory_store() {
    use async_trait::async_trait;
    use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
    use sepia_core::{Session, StorageError};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Mem {
        sessions: Mutex<HashMap<String, Session>>,
    }
    #[async_trait]
    impl SessionRepository for Mem {
        async fn save(&self, s: &Session) -> Result<(), StorageError> {
            self.sessions
                .lock()
                .unwrap()
                .insert(s.id.clone(), s.clone());
            Ok(())
        }
        async fn get_by_id(
            &self,
            id: &str,
            _a: Option<&str>,
        ) -> Result<Option<Session>, StorageError> {
            Ok(self.sessions.lock().unwrap().get(id).cloned())
        }
        async fn list(&self) -> Result<Vec<Session>, StorageError> {
            Ok(self.sessions.lock().unwrap().values().cloned().collect())
        }
        async fn delete(&self, id: &str) -> Result<(), StorageError> {
            self.sessions.lock().unwrap().remove(id);
            Ok(())
        }
        async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
            Ok(self.sessions.lock().unwrap().contains_key(id))
        }
        async fn summary(
            &self,
            id: &str,
            a: Option<&str>,
        ) -> Result<Option<Session>, StorageError> {
            Ok(self.get_by_id(id, a).await?.map(|mut s| {
                s.nodes = Vec::new();
                s
            }))
        }
        async fn nodes_window(
            &self,
            id: &str,
            o: &NodesWindowOptions,
        ) -> Result<Option<SessionNodeWindow>, StorageError> {
            Ok(self.sessions.lock().unwrap().get(id).map(|s| {
                let total = s.nodes.len();
                let before = o.before.map_or(total, |b| {
                    usize::try_from(b.max(0)).unwrap_or(usize::MAX).min(total)
                });
                let start = before.saturating_sub(o.limit.unwrap_or(total));
                SessionNodeWindow {
                    nodes: s.nodes[start..before].to_vec(),
                    tool_call_nodes: Vec::new(),
                    total,
                    start,
                    backend_type: s.backend_type.clone(),
                }
            }))
        }
    }

    let store = Mem {
        sessions: Mutex::new(HashMap::new()),
    };
    sepia_testkit::assert_session_repository_contract(&store).await;
}

#[allow(dead_code)]
fn _v(_: Value) {}
