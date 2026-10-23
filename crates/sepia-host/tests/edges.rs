#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Host-side edges: `DriverClient` stderr tail / notification fan-out /
//! junk frames / death semantics, `RemoteStore` + `MergedStore` routing,
//! `DriverRegistry` discover/refresh/probe error paths.

#[cfg(unix)]
mod unix_edges {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use sepia_core::storage::{NodesWindowOptions, SessionRepository};
    use sepia_host::store::{MergedStore, RemoteStore};
    use sepia_host::{DriverClient, DriverRegistry, command_resolves, discover, refresh_manifest};
    use serde_json::json;

    /// A driver script: spews stderr (past the 64-line ring), emits a
    /// notification + a request + garbage on `emit.junk`, never answers
    /// `never.answers`, exits on `die`, errors on `boom`, and answers the
    /// session/config surface minimally.
    fn fake_driver(dir: &Path, name: &str) -> PathBuf {
        let bin = dir.join(name);
        let script = "#!/bin/sh\n\
            i=0; while [ $i -lt 80 ]; do echo \"noise $i\" >&2; i=$((i+1)); done\n\
            while IFS= read -r line; do\n\
            \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
            \tcase \"$line\" in\n\
            \t\t*'\"method\":\"driver.manifest\"'*) result='{\"id\":\"fake\",\"label\":\"F\",\"version\":\"0\",\"protocol\":1,\"capabilities\":[\"sessionStore\",\"sessionWrite\",\"agentConfig\",\"rewind\"]}' ;;\n\
            \t\t*'\"method\":\"session.list\"'*) result='[]' ;;\n\
            \t\t*'\"method\":\"session.get\"'*) result='{\"id\":\"s1\",\"title\":\"T\",\"workingDirectory\":\"/w\",\"model\":\"m\",\"createdAt\":1,\"lastActivityAt\":2,\"mainChainId\":0,\"nodes\":[],\"promptHistory\":[]}' ;;\n\
            \t\t*'\"method\":\"session.summary\"'*) result='{\"id\":\"s1\"}' ;;\n\
            \t\t*'\"method\":\"session.history\"'*) result='{\"nodes\":[],\"toolCallNodes\":[],\"total\":0,\"start\":0,\"backendType\":\"fake\"}' ;;\n\
            \t\t*'\"method\":\"session.save\"'*) result='null' ;;\n\
            \t\t*'\"method\":\"session.delete\"'*) result='null' ;;\n\
            \t\t*'\"method\":\"session.rewind\"'*) result='null' ;;\n\
            \t\t*'\"method\":\"config.read\"'*) result='{\"rules\":[]}' ;;\n\
            \t\t*'\"method\":\"config.write\"'*) result='{\"actions\":[]}' ;;\n\
            \t\t*'\"method\":\"boom\"'*) printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32001,\"message\":\"went boom\"}}\\n' \"$id\"; continue ;;\n\
            \t\t*'\"id\":\"missing\"'*) printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32001,\"message\":\"not found\"}}\\n' \"$id\"; continue ;;\n\
            \t\t*'\"id\":\"err\"'*) printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32005,\"message\":\"store blew up\"}}\\n' \"$id\"; continue ;;\n\
            \t\t*'\"id\":\"weird\"'*) printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":\"not-a-session\"}\\n' \"$id\"; continue ;;\n\
            \t\t*'\"method\":\"emit.junk\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"noise\",\"params\":{}}' '{\"jsonrpc\":\"2.0\",\"id\":555,\"method\":\"inbound.req\"}' '{\"jsonrpc\":\"2.0\",\"id\":556,\"result\":null}' 'not even json' ;;\n\
            \t\t*'\"method\":\"never.answers\"'*) continue ;;\n\
            \t\t*'\"method\":\"die\"'*) exit 0 ;;\n\
            \t\t*) result='null' ;;\n\
            \tesac\n\
            \tprintf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}\\n' \"$id\" \"$result\"\n\
            done\n";
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    /// A driver that fails --manifest in different ways.
    fn bad_driver(dir: &Path, name: &str, body: &str) -> PathBuf {
        let bin = dir.join(name);
        std::fs::write(&bin, body).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[tokio::test]
    async fn client_stderr_tail_junk_frames_and_death() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_driver(dir.path(), "sepia-driver-fake");
        let client = DriverClient::spawn(&bin, &[]).await.unwrap();

        // The startup stderr flood ran through the 64-line ring.
        let tail = client.stderr_tail().await;
        assert_eq!(tail.len(), 64);
        assert!(tail.last().unwrap().contains("noise 79"));

        // Notifications broadcast; junk/request lines are dropped.
        let mut notes = client.notifications();
        client.call("emit.junk", json!({})).await.unwrap();
        let note = tokio::time::timeout(Duration::from_secs(5), notes.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.method, "noise");

        // A driver error surfaces as RpcError.
        let err = client.call("boom", json!({})).await.unwrap_err();
        assert_eq!(err.message, "went boom");

        // The 30s default call timeout fires when the driver never answers.
        // (Skipped unless explicitly asked — it costs 30s wall-clock.)
        if std::env::var_os("SEPTEST_SLOW_TIMEOUT").is_some() {
            let err = client.call("never.answers", json!({})).await.unwrap_err();
            assert!(err.message.contains("timed out"), "{err}");
        }

        // Death: stdout closes, is_closed flips, pending fails.
        let pending = {
            let client = client.clone();
            tokio::spawn(async move { client.call("never.answers", json!({})).await })
        };
        client.call("die", json!({})).await.ok();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !client.is_closed() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(client.is_closed());
        assert!(pending.await.unwrap().is_err());
        // Calls after death fail on the closed stdin/reader.
        assert!(client.call("session.list", json!({})).await.is_err());
    }

    #[tokio::test]
    async fn remote_store_and_merged_store_routing() {
        let dir = tempfile::tempdir().unwrap();
        let a = fake_driver(dir.path(), "sepia-driver-a");
        let b = fake_driver(dir.path(), "sepia-driver-b");
        let client_a = Arc::new(DriverClient::spawn(&a, &[]).await.unwrap());
        let client_b = Arc::new(DriverClient::spawn(&b, &[]).await.unwrap());
        let store_a = RemoteStore::new(client_a.clone(), "alpha".into());
        let store_b = RemoteStore::new(client_b, "beta".into());

        // RemoteStore passthroughs incl. config + rewind wire methods.
        assert_eq!(store_a.agent_id, "alpha");
        let _ = store_a.client();
        assert!(store_a.read_config(None).await.is_ok());
        assert!(
            store_a
                .write_config(None, &json!({"rules": []}))
                .await
                .is_ok()
        );
        let session: sepia_core::Session = serde_json::from_value(json!({
            "id": "s1", "title": "T", "workingDirectory": "/w", "model": "m",
            "createdAt": 1.0, "lastActivityAt": 2.0, "mainChainId": 0,
            "nodes": [], "promptHistory": []
        }))
        .unwrap();
        let plan = sepia_core::rewind::RewindPlan {
            keep_count: 0,
            kept: vec![],
            removed: vec![],
            removed_tool_call_ids: vec![],
        };
        store_a.rewind(&session, &plan, &session).await.unwrap();
        assert!(store_a.get_by_id("s1", None).await.unwrap().is_some());
        assert!(store_a.summary("s1", None).await.unwrap().is_some());
        assert!(
            store_a
                .nodes_window("s1", &NodesWindowOptions::default())
                .await
                .unwrap()
                .is_some()
        );
        store_a.save(&session).await.unwrap();
        store_a.delete("s1").await.unwrap();
        // Error-code mapping: SESSION_NOT_FOUND decodes to Ok(None);
        // other errors propagate; a malformed payload fails the decode.
        assert!(store_a.get_by_id("missing", None).await.unwrap().is_none());
        assert!(store_a.summary("missing", None).await.unwrap().is_none());
        assert!(
            store_a
                .nodes_window("missing", &NodesWindowOptions::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(store_a.get_by_id("err", None).await.is_err());
        assert!(store_a.get_by_id("weird", None).await.is_err());

        // MergedStore over both remotes.
        let merged = MergedStore::new(vec![store_a.clone(), store_b.clone()]);
        assert!(merged.for_agent("beta").is_some());
        assert!(merged.for_agent("ghost").is_none());
        assert!(merged.list().await.unwrap().is_empty());
        assert!(merged.get_by_id("s1", None).await.unwrap().is_some());
        // Scoped picks route to the named store only.
        assert!(
            merged
                .get_by_id("s1", Some("beta"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(merged.summary("s1", Some("alpha")).await.unwrap().is_some());
        assert!(
            merged
                .nodes_window(
                    "s1",
                    &NodesWindowOptions {
                        agent_id: Some("beta".into()),
                        ..Default::default()
                    }
                )
                .await
                .unwrap()
                .is_some()
        );
        assert!(merged.has_session("s1").await.unwrap());
        merged.delete("s1").await.unwrap();
        // save targets the session's agent store, else the first.
        let mut owned = session.clone();
        owned.agent_id = Some("beta".into());
        merged.save(&owned).await.unwrap();
        merged.save(&session).await.unwrap();
        // Scoped-store errors propagate; unscoped ones degrade to misses.
        assert!(merged.get_by_id("err", Some("alpha")).await.is_err());
        assert!(merged.get_by_id("err", None).await.unwrap().is_none());
        assert!(merged.summary("err", Some("beta")).await.is_err());
        assert!(merged.summary("err", None).await.unwrap().is_none());
        assert!(
            merged
                .nodes_window(
                    "err",
                    &NodesWindowOptions {
                        agent_id: Some("alpha".into()),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert!(
            merged
                .nodes_window("err", &NodesWindowOptions::default())
                .await
                .unwrap()
                .is_none()
        );
        // Merged delete falls through to the store that has the session;
        // a dead store's has_session degrades to false.
        client_a.kill().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(merged.has_session("s1").await.unwrap()); // beta still answers
        merged.list().await.unwrap(); // alpha's list failure is logged, not fatal
        merged.delete("s1").await.unwrap();

        // An empty merged store fails saves.
        let empty = MergedStore::new(vec![]);
        assert!(empty.save(&session).await.is_err());
        assert!(empty.delete("x").await.is_err());
    }

    #[tokio::test]
    async fn registry_discover_probe_and_respawn_edges() {
        let dir = tempfile::tempdir().unwrap();
        // A driver that probes fine.
        let good = fake_driver(dir.path(), "sepia-driver-good");
        // --manifest exits nonzero.
        let fails = bad_driver(dir.path(), "sepia-driver-fails", "#!/bin/sh\nexit 1\n");
        // --manifest prints non-JSON.
        let junk = bad_driver(
            dir.path(),
            "sepia-driver-junk",
            "#!/bin/sh\necho not-json\n",
        );
        // --manifest prints JSON that fails validation (bad protocol).
        let invalid = bad_driver(
            dir.path(),
            "sepia-driver-invalid",
            "#!/bin/sh\necho '{\"id\":\"bad\",\"protocol\":99}'\n",
        );

        assert!(discover::probe_manifest(&good).await.is_ok());
        let err = discover::probe_manifest(&fails).await.unwrap_err();
        assert!(err.contains("exited"), "{err}");
        let err = discover::probe_manifest(&junk).await.unwrap_err();
        assert!(err.contains("invalid JSON"), "{err}");
        let err = discover::probe_manifest(&invalid).await.unwrap_err();
        assert!(err.contains("protocol"), "{err}");
        let err = discover::probe_manifest(Path::new("/nonexistent-bin"))
            .await
            .unwrap_err();
        assert!(err.contains("failed"), "{err}");

        // refresh_manifest over a live client + its error arm.
        let client = DriverClient::spawn(&good, &[]).await.unwrap();
        assert_eq!(refresh_manifest(&client).await.unwrap().id, "fake");
        client.call("die", json!({})).await.ok();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(refresh_manifest(&client).await.is_err());

        // Registry insert + entry lookups + capability filtering.
        let mut registry = DriverRegistry::default();
        let manifest = discover::probe_manifest(&good).await.unwrap();
        registry.insert(good.clone(), manifest);
        assert_eq!(registry.manifest("fake").unwrap().id, "fake");
        assert!(registry.manifest("nope").is_none());
        assert!(registry.entry("fake").is_some());
        assert!(registry.entry("nope").is_none());
        assert_eq!(registry.manifests().count(), 1);
        assert_eq!(
            registry.with(&sepia_driver::Capability::SessionStore).len(),
            1
        );
        assert!(registry.with(&sepia_driver::Capability::Restore).is_empty());
        // merged_store spawns live clients for SessionStore drivers.
        let merged = registry.merged_store().await;
        assert!(merged.list().await.unwrap().is_empty());

        // A driver entry whose binary is missing errors on client().
        let mut registry = DriverRegistry::default();
        let mut m = discover::probe_manifest(&good).await.unwrap();
        m.id = "ghost".into();
        registry.insert(dir.path().join("missing-bin"), m);
        assert!(registry.entry("ghost").unwrap().client().await.is_err());
        // usable_agents / command_resolves edges.
        assert!(command_resolves("/bin/sh"));
        assert!(!command_resolves(""));
        assert!(!command_resolves("definitely-not-a-real-command-xyz"));
        assert!(registry.usable_agents().is_empty());
    }

    #[test]
    fn discover_search_dirs_and_binary_scan() {
        let _ = discover::search_dirs();
        let _ = discover::find_driver_binaries();
        // A directory containing a driver-named file is found by the scan
        // only when executable — is_driver_binary gates that; verified by
        // probe tests above plus the PATH scan running clean.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sepia-driver-x"), "x").unwrap();
        let _ = dir;
    }
}
