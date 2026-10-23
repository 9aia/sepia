#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `Local::import_session` / `Local::convert_session` — the store-write
//! seam behind `sessions.import`/`sessions.convert` — exercised against
//! a real spawned `sepia-driver-*` shell script so `session.save`
//! genuinely crosses the driver wire.

mod common;

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use sepia_core::Session;
    use sepia_core::storage::SessionRepository;
    use sepia_driver::{Capability, DRIVER_PROTOCOL, DriverManifest};
    use sepia_host::DriverRegistry;
    use sepia_session::{
        ControlErrorCode, ControlPlane, ControlPlaneOptions, ImportOverrides, Local, MetaStore,
        NodeIdentity, Paths,
    };
    use sepia_testkit::contract;
    use serde_json::{Value, json};

    use crate::common::*;

    /// A fake store driver: every request gets `result:null` and each
    /// `session.save` request line is appended to a log file the test
    /// asserts on. Request ids always lead the line (`{"jsonrpc":…,"id":N`)
    /// so the `sed` extraction is safe.
    fn fake_driver(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
        let log = dir.join(format!("{name}.saves.ndjson"));
        let bin = dir.join(format!("sepia-driver-{name}"));
        let script = format!(
            "#!/bin/sh\n\
             while IFS= read -r line; do\n\
             \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
             \tcase \"$line\" in\n\
             \t\t*'\"method\":\"session.save\"'*) printf '%s\\n' \"$line\" >> '{log}' ;;\n\
             \tesac\n\
             \tprintf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":null}}\\n' \"$id\"\n\
             done\n",
            log = log.display()
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    fn manifest(id: &str, write: bool) -> DriverManifest {
        DriverManifest {
            id: id.into(),
            label: id.into(),
            version: "0.0.1".into(),
            protocol: DRIVER_PROTOCOL,
            capabilities: if write {
                [Capability::SessionStore, Capability::SessionWrite]
                    .into_iter()
                    .collect()
            } else {
                [Capability::SessionStore].into_iter().collect()
            },
            agent_command: None,
            config_schema: json!({}),
            backend_type: Some(id.into()),
        }
    }

    /// A `Local` hand-assembled like `build()` would, minus discovery —
    /// the registry carries only the test's fake drivers and the plane
    /// reads `MemStore` instead of a merged remote store.
    fn mk_local(
        sessions: Vec<Session>,
        registry: DriverRegistry,
        home: &Path,
    ) -> (Local, Arc<MemStore>) {
        let store = Arc::new(MemStore::new(sessions));
        let repo: Arc<dyn SessionRepository> = store.clone();
        let plane = ControlPlane::new(Arc::clone(&repo), ControlPlaneOptions::default());
        let paths = Paths::new(home.to_path_buf());
        (
            Local {
                plane,
                meta: MetaStore::open(&paths.meta_path),
                identity: NodeIdentity {
                    id: "node_test".into(),
                    name: "test".into(),
                },
                registry,
                repo,
                paths,
            },
            store,
        )
    }

    /// The `session.save` params the fake driver recorded, decoded.
    fn saved_sessions(log: &Path) -> Vec<Session> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let request: Value = serde_json::from_str(line).unwrap();
                assert_eq!(request["method"], "session.save");
                serde_json::from_value(request["params"].clone()).unwrap()
            })
            .collect()
    }

    #[tokio::test]
    async fn import_mints_a_cline_shaped_id_and_saves_over_the_wire() {
        let home = tempdir();
        let (bin, log) = fake_driver(home.path(), "cline");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("cline", true));
        let (local, _) = mk_local(vec![], registry, home.path());
        let session = contract::session("src1", "Source", 10.0);

        let id = local
            .import_session("cline", &session, &ImportOverrides::default())
            .await
            .unwrap();

        // cline's `<epoch-ms>_<5 alnum>` convention, minted fresh.
        assert_ne!(id, session.id);
        let (stamp, suffix) = id.split_once('_').unwrap();
        assert_eq!(stamp, "1700000000000", "{id}");
        assert_eq!(suffix.len(), 5, "{id}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );

        let saved = saved_sessions(&log);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        assert_eq!(saved[0].title, "Source");
        assert_eq!(saved[0].nodes.len(), session.nodes.len());
    }

    #[tokio::test]
    async fn import_applies_overrides_and_a_caller_chosen_id() {
        let home = tempdir();
        let (bin, log) = fake_driver(home.path(), "devin");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("devin", true));
        let (local, _) = mk_local(vec![], registry, home.path());
        let session = contract::session("src1", "Source", 10.0);

        let overrides = ImportOverrides {
            id: Some("chosen-1".into()),
            cwd: Some("/elsewhere".into()),
            title: Some("Renamed".into()),
            model: Some("gpt-test".into()),
        };
        let id = local
            .import_session("devin", &session, &overrides)
            .await
            .unwrap();

        assert_eq!(id, "chosen-1");
        let saved = saved_sessions(&log);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "chosen-1");
        assert_eq!(saved[0].working_directory, "/elsewhere");
        assert_eq!(saved[0].title, "Renamed");
        assert_eq!(saved[0].model, "gpt-test");
    }

    #[tokio::test]
    async fn import_mints_a_uuid_for_stores_without_an_id_convention() {
        let home = tempdir();
        let (bin, _) = fake_driver(home.path(), "devin");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("devin", true));
        let (local, _) = mk_local(vec![], registry, home.path());
        let session = contract::session("src1", "Source", 10.0);

        let id = local
            .import_session("devin", &session, &ImportOverrides::default())
            .await
            .unwrap();

        assert_ne!(id, session.id);
        assert_eq!(id.len(), 36, "{id}");
    }

    #[tokio::test]
    async fn import_errors_unknown_agent_for_missing_or_read_only_drivers() {
        let home = tempdir();
        let (bin, _) = fake_driver(home.path(), "reader");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("reader", false));
        let (local, _) = mk_local(vec![], registry, home.path());
        let session = contract::session("src1", "Source", 10.0);

        let err = local
            .import_session("ghost", &session, &ImportOverrides::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);

        let err = local
            .import_session("reader", &session, &ImportOverrides::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    }

    #[tokio::test]
    async fn convert_copies_a_stored_session_into_the_target_store() {
        let home = tempdir();
        let (bin, log) = fake_driver(home.path(), "fake");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        let session = session_with("fake", vec![node(0, sepia_core::Role::User, "hi")]);
        let (local, store) = mk_local(vec![session.clone()], registry, home.path());

        let id = local.convert_session("s1", "fake", None).await.unwrap();

        assert_ne!(id, "s1");
        let saved = saved_sessions(&log);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        assert_eq!(saved[0].nodes.len(), 1);
        // The source copy is untouched.
        assert!(store.has_session("s1").await.unwrap());
        local.plane.close_all().await;
    }

    #[tokio::test]
    async fn convert_unknown_session_is_not_found() {
        let home = tempdir();
        let (bin, _) = fake_driver(home.path(), "fake");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        let (local, _) = mk_local(vec![], registry, home.path());

        let err = local
            .convert_session("nope", "fake", None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::NotFound);
        local.plane.close_all().await;
    }
}

#[cfg(not(unix))]
mod imp {}
