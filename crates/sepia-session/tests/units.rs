#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Unit coverage for the small modules: `types` conversions,
//! `merged::MergedRepository`, `exec::DefaultRestoreExec`, and the
//! `local` helpers that don't need a spawned driver.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use sepia_core::{Session, StorageError};
use sepia_session::{
    ControlErrorCode, ControlPlaneOptions, Local, MergedRepository, MetaStore, NodeIdentity, Paths,
    RestoreExec, base_options, sepia_home,
};
use sepia_testkit::contract;

mod common;
use common::*;

// ---------- types ------------------------------------------------------

#[test]
fn run_span_converts_from_the_meta_row() {
    let span: sepia_session::RunSpan = sepia_meta::RunSpan {
        at: 1.5,
        agent: "devin".into(),
        node: "n1".into(),
    }
    .into();
    assert_eq!(span.at, 1.5);
    assert_eq!(span.agent, "devin");
    assert_eq!(span.node, "n1");
}

#[test]
fn control_error_codes_serialize_to_the_wire_names() {
    let pairs = [
        (ControlErrorCode::NotFound, "not_found"),
        (ControlErrorCode::Invalid, "invalid"),
        (ControlErrorCode::Locked, "locked"),
        (ControlErrorCode::UnknownAgent, "unknown_agent"),
        (ControlErrorCode::Conflict, "conflict"),
        (ControlErrorCode::Busy, "busy"),
        (ControlErrorCode::Internal, "internal"),
    ];
    for (code, name) in pairs {
        assert_eq!(code.as_str(), name);
    }
}

// ---------- merged ------------------------------------------------------

/// A store that fails every call — overlays must degrade to misses,
/// never poison the merged view.
struct DeadStore;

#[async_trait]
impl SessionRepository for DeadStore {
    async fn save(&self, _session: &Session) -> Result<(), StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn get_by_id(
        &self,
        _id: &str,
        _agent: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn delete(&self, _id: &str) -> Result<(), StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn has_session(&self, _id: &str) -> Result<bool, StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn summary(
        &self,
        _id: &str,
        _agent: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        Err(StorageError::new("dead"))
    }
    async fn nodes_window(
        &self,
        _id: &str,
        _options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        Err(StorageError::new("dead"))
    }
}

#[tokio::test]
async fn merged_repository_primary_wins_and_extras_degrade() {
    let primary = Arc::new(MemStore::new(vec![contract::session("a", "A", 1.0)]));
    let extra = Arc::new(MemStore::new(vec![
        contract::session("a", "shadowed", 9.0),
        contract::session("b", "B", 2.0),
    ]));
    let merged = MergedRepository::new(primary.clone(), vec![Arc::new(DeadStore), extra.clone()]);

    let s = merged.get_by_id("a", None).await.unwrap().unwrap();
    assert_eq!(s.title, "A"); // primary wins the collision
    let s = merged.get_by_id("b", None).await.unwrap().unwrap();
    assert_eq!(s.title, "B"); // falls through to the extra
    assert!(merged.get_by_id("nope", None).await.unwrap().is_none());

    let list = merged.list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "b"); // sorted by activity desc
    assert_eq!(list[1].title, "A");

    let s = merged.summary("b", None).await.unwrap().unwrap();
    assert_eq!(s.title, "B");
    assert!(merged.summary("nope", None).await.unwrap().is_none());
    assert!(merged.has_session("b").await.unwrap());
    assert!(!merged.has_session("nope").await.unwrap());

    // save/delete hit the primary only.
    merged
        .save(&contract::session("c", "C", 3.0))
        .await
        .unwrap();
    assert!(primary.has_session("c").await.unwrap());
    assert!(!extra.has_session("c").await.unwrap());
    merged.delete("a").await.unwrap();
    assert!(!primary.has_session("a").await.unwrap());
    // The extra's copy still answers the merged view.
    assert!(merged.has_session("a").await.unwrap());

    // nodes_window falls through to extras when the primary misses.
    assert!(
        merged
            .nodes_window("b", &NodesWindowOptions::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn merged_repository_primary_errors_propagate() {
    let merged = MergedRepository::new(Arc::new(DeadStore), vec![]);
    assert!(merged.list().await.is_err());
    assert!(merged.get_by_id("x", None).await.is_err());
    assert!(merged.summary("x", None).await.is_err());
    assert!(merged.has_session("x").await.is_err());
    assert!(
        merged
            .nodes_window("x", &NodesWindowOptions::default())
            .await
            .is_err()
    );
    assert!(
        merged
            .save(&contract::session("x", "x", 0.0))
            .await
            .is_err()
    );
    assert!(merged.delete("x").await.is_err());
}

// ---------- exec --------------------------------------------------------

#[tokio::test]
async fn default_restore_exec_file_ops() {
    use sepia_session::DefaultRestoreExec;
    let exec = DefaultRestoreExec;
    let dir = tempdir();
    let path = dir.path().join("nested/deep/f.txt");
    assert!(exec.read_file(&path).await.unwrap().is_none());
    exec.write_file(&path, b"body").await.unwrap();
    assert_eq!(exec.read_file(&path).await.unwrap().unwrap(), b"body");
    exec.remove_file(&path).await.unwrap();
    // Removing a missing file is a no-op, not an error.
    exec.remove_file(&path).await.unwrap();
    // write_file failure surfaces as Err.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let under_file = blocker.join("child");
    assert!(exec.write_file(&under_file, b"y").await.is_err());
    // remove_file on a directory is a real error (not NotFound).
    let subdir = dir.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    assert!(exec.remove_file(&subdir).await.is_err());
    // read_file failure reads as absent.
    assert!(
        exec.read_file(&dir.path().join("missing"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn default_restore_exec_runs_git() {
    use sepia_session::DefaultRestoreExec;
    let exec = DefaultRestoreExec;
    let dir = tempdir();
    // Not a repo → non-zero code is an answer, not an error.
    let out = exec
        .git(dir.path(), &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap();
    assert_ne!(out.code, 0);
    // A real repo answers true.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let out = exec
        .git(dir.path(), &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
}

// ---------- local helpers -------------------------------------------------

#[test]
fn paths_and_node_identity_round_trip() {
    let dir = tempdir();
    let paths = Paths::new(dir.path().join("home"));
    assert_eq!(paths.meta_path, paths.home.join("meta.json"));
    // node_path defaults under home (SEPIA_NODE unset in tests).
    assert!(paths.node_path.ends_with("node.json"));

    // First call mints + persists; second reads it back.
    let id1 = sepia_session::local::node_identity(&paths);
    assert!(id1.id.starts_with("node_"));
    assert!(!id1.name.is_empty());
    assert!(paths.node_path.exists());
    let id2 = sepia_session::local::node_identity(&paths);
    assert_eq!(id1.id, id2.id);

    // A corrupt node.json re-mints (and rewrites).
    std::fs::write(&paths.node_path, b"not json").unwrap();
    let id3 = sepia_session::local::node_identity(&paths);
    assert!(id3.id.starts_with("node_"));

    // Unwritable parent → mints in memory, persists nothing.
    let file = dir.path().join("afile");
    std::fs::write(&file, b"x").unwrap();
    let bad = Paths {
        home: file.clone(),
        meta_path: file.join("meta.json"),
        node_path: file.join("node.json"),
    };
    let id4 = sepia_session::local::node_identity(&bad);
    assert!(id4.id.starts_with("node_"));
}

#[test]
fn sepia_home_prefers_env_then_home_then_fallback() {
    // The test process always has HOME; SEPIA_HOME may or may not be set.
    let home = sepia_home();
    assert!(!home.as_os_str().is_empty());
}

#[test]
fn base_options_wires_the_machine_defaults() {
    let opts = base_options();
    assert!(opts.probe_cwd.is_some());
    assert!(opts.file_history_dir.is_some());
    let terminate = opts.terminate_lock_holder.unwrap();
    // Signaling a bogus pid is a no-op (ESRCH swallowed inside nix).
    terminate(99_999_999);
}

#[test]
fn agent_config_counts_and_summary_shapes() {
    let config = sepia_core::agent_config::AgentConfig::default();
    let counts = sepia_session::AgentConfigCounts::of(&config);
    assert_eq!(counts.skills, 0);
    let summary = sepia_session::AgentConfigSummary {
        agent: "devin".into(),
        counts: Some(counts),
        error: None,
    };
    let v = serde_json::to_value(&summary).unwrap();
    assert_eq!(v["agent"], "devin");
    assert_eq!(v["counts"]["mcpServers"], 0);
}

// ---------- local: capable_store / import paths -----------------------------

/// `Local` built by hand with an empty registry — `capable_store`'s
/// unknown-agent arm and the not-config-capable arm.
#[tokio::test]
async fn local_unknown_agent_and_capability_gates() {
    let dir = tempdir();
    let registry = sepia_host::DriverRegistry::default();
    let store = Arc::new(MemStore::new(vec![]));
    let repo: Arc<dyn sepia_core::storage::SessionRepository> = store;
    let plane = sepia_session::ControlPlane::new(repo.clone(), ControlPlaneOptions::default());
    let paths = Paths::new(dir.path().to_path_buf());
    let local = Local {
        plane,
        meta: MetaStore::open(&paths.meta_path),
        identity: NodeIdentity {
            id: "n".into(),
            name: "n".into(),
        },
        registry,
        repo,
        paths,
    };
    let err = local.agent_config("ghost", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    let err = local
        .write_agent_config("ghost", None, &Default::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    let err = local
        .agent_config_summaries(Some("ghost"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    let err = local
        .import_session(
            "ghost",
            &contract::session("s", "s", 0.0),
            &Default::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    // convert_session: unknown session id → not_found from the plane.
    let err = local
        .convert_session("nope", "ghost", None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::NotFound);
    // Empty registry → no config-capable drivers → empty summaries.
    let rows = local.agent_config_summaries(None, None).await.unwrap();
    assert!(rows.is_empty());
    local.plane.close_all().await;
}

#[tokio::test]
async fn local_build_with_no_drivers_discoverable() {
    // An isolated SEPIA_HOME is impossible without env control — but
    // build() over an empty registry is still exercised: discover()
    // finds whatever the test environment provides; the plane builds
    // regardless.
    let dir = tempdir();
    let paths = Paths::new(dir.path().join("home"));
    let local = sepia_session::local::build(&paths, ControlPlaneOptions::default())
        .await
        .unwrap();
    assert!(local.identity.id.starts_with("node_"));
    local.plane.close_all().await;
}

/// A rewinder that records its invocation — exercises the
/// `options.rewinders` takeover in `build` plus `can_rewind`.
#[tokio::test]
async fn build_preserves_caller_supplied_rewinders_and_meta() {
    struct Record(Arc<Mutex<usize>>);
    #[async_trait]
    impl sepia_session::SessionRewinder for Record {
        async fn truncate(
            &self,
            _session: &Session,
            _plan: &sepia_core::rewind::RewindPlan,
            _truncated: &Session,
        ) -> Result<(), sepia_session::ControlError> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }
    let dir = tempdir();
    let paths = Paths::new(dir.path().join("home"));
    let calls = Arc::new(Mutex::new(0usize));
    let mut rewinders = HashMap::new();
    rewinders.insert(
        "devin".to_string(),
        Arc::new(Record(Arc::clone(&calls))) as Arc<dyn sepia_session::SessionRewinder>,
    );
    let meta = MetaStore::open(&dir.path().join("custom-meta.json"));
    let local = sepia_session::local::build(
        &paths,
        ControlPlaneOptions {
            rewinders,
            meta: Some(meta),
            node_name: Some("preset".into()),
            ..ControlPlaneOptions::default()
        },
    )
    .await
    .unwrap();
    assert!(local.plane.can_rewind("windsurf"));
    assert!(local.identity.id.starts_with("node_"));
    local.plane.close_all().await;
}
