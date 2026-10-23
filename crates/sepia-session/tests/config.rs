#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `Local::agent_config` / `write_agent_config` / `agent_config_summaries`
//! — the `config.*` seam behind `config.get`/`config.import`/`config.list`
//! — exercised against a real spawned `sepia-driver-*` shell script so
//! `config.read`/`config.write` genuinely cross the driver wire.

mod common;

#[cfg(unix)]
mod cfg {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use sepia_core::agent_config::{
        AgentConfig, ConfigRule, RuleKind, WriteActionKind, config_to_json,
    };
    use sepia_core::storage::SessionRepository;
    use sepia_driver::{Capability, DRIVER_PROTOCOL, DriverManifest};
    use sepia_host::DriverRegistry;
    use sepia_session::{
        ControlErrorCode, ControlPlane, ControlPlaneOptions, Local, MetaStore, NodeIdentity, Paths,
    };
    use serde_json::{Value, json};

    use crate::common::*;

    /// A canned config IR payload the fake driver answers `config.read`
    /// with.
    const CONFIG: &str = r#"{"rules":[{"name":"house","kind":"rule"}],"mcpServers":{"docs":{"command":"mcp-docs"}},"metadata":{"source":"fake","dir":"/work"}}"#;

    /// A fake config driver: `config.read` requests are logged to
    /// `reads.ndjson` and answered with `CONFIG`; `config.write`
    /// requests land in `writes.ndjson` and answer one `wrote` action.
    fn fake_driver(dir: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let reads = dir.join(format!("{name}.reads.ndjson"));
        let writes = dir.join(format!("{name}.writes.ndjson"));
        let bin = dir.join(format!("sepia-driver-{name}"));
        let script = format!(
            "#!/bin/sh\n\
             while IFS= read -r line; do\n\
             \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
             \tcase \"$line\" in\n\
             \t\t*'\"method\":\"config.read\"'*) printf '%s\\n' \"$line\" >> '{reads}'; result='{CONFIG}' ;;\n\
             \t\t*'\"method\":\"config.write\"'*) printf '%s\\n' \"$line\" >> '{writes}'; result='{{\"actions\":[{{\"path\":\"/out/r.md\",\"action\":\"wrote\"}}]}}' ;;\n\
             \t\t*) result='null' ;;\n\
             \tesac\n\
             \tprintf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}}\\n' \"$id\" \"$result\"\n\
             done\n",
            reads = reads.display(),
            writes = writes.display(),
            CONFIG = CONFIG,
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, reads, writes)
    }

    fn manifest(id: &str, config_capable: bool) -> DriverManifest {
        DriverManifest {
            id: id.into(),
            label: id.into(),
            version: "0.0.1".into(),
            protocol: DRIVER_PROTOCOL,
            capabilities: if config_capable {
                [Capability::SessionStore, Capability::AgentConfig]
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

    /// A `Local` hand-assembled like `build()` would — the registry
    /// carries only the test's fake drivers.
    fn mk_local(registry: DriverRegistry, home: &Path) -> Local {
        let store = Arc::new(MemStore::new(vec![]));
        let repo: Arc<dyn SessionRepository> = store;
        let plane = ControlPlane::new(repo.clone(), ControlPlaneOptions::default());
        let paths = Paths::new(home.to_path_buf());
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
        }
    }

    fn rule(name: &str) -> ConfigRule {
        ConfigRule {
            name: name.into(),
            description: None,
            body: String::new(),
            globs: Vec::new(),
            always_apply: true,
            kind: RuleKind::Rule,
            metadata: Value::Null,
        }
    }

    /// The params of each logged request line, decoded.
    fn logged_params(log: &Path) -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["params"].clone())
            .collect()
    }

    #[tokio::test]
    async fn agent_config_reads_the_ir_over_the_wire() {
        let home = tempdir();
        let (bin, reads, _) = fake_driver(home.path(), "fake");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        let local = mk_local(registry, home.path());

        let config = local.agent_config("fake", Some("/work")).await.unwrap();
        assert_eq!(config.rules[0].name, "house");
        assert_eq!(config.mcp_servers["docs"]["command"], "mcp-docs");
        assert_eq!(config.metadata["dir"], "/work");

        // The `dir` param rode the wire.
        let sent = logged_params(&reads);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["dir"], "/work");
        local.plane.close_all().await;
    }

    #[tokio::test]
    async fn write_agent_config_sends_the_ir_and_returns_actions() {
        let home = tempdir();
        let (bin, _, writes) = fake_driver(home.path(), "fake");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        let local = mk_local(registry, home.path());

        let config = AgentConfig {
            rules: vec![rule("installed")],
            ..AgentConfig::default()
        };
        let actions = local
            .write_agent_config("fake", Some("/work"), &config)
            .await
            .unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].path, "/out/r.md");
        assert_eq!(actions[0].action, WriteActionKind::Wrote);
        let sent = logged_params(&writes);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["dir"], "/work");
        assert_eq!(sent[0]["config"]["rules"][0]["name"], "installed");
        local.plane.close_all().await;
    }

    #[tokio::test]
    async fn config_write_accepts_the_export_shape_round_trip() {
        // `config_to_json` output (the `config export` payload) is what
        // `write_agent_config` serializes back — a lossless IR hop.
        let home = tempdir();
        let (bin, _, writes) = fake_driver(home.path(), "fake");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        let local = mk_local(registry, home.path());

        let mut config = AgentConfig::default();
        config.rules.push(rule("x"));
        local
            .write_agent_config("fake", None, &config)
            .await
            .unwrap();
        let sent = logged_params(&writes);
        // `dir` serializes as null → the driver resolves its default.
        assert!(sent[0]["dir"].is_null());
        // The wire payload is the serialized IR — `config_to_json`'s
        // inverse handles it.
        assert_eq!(
            sent[0]["config"]["rules"][0]["name"],
            config_to_json(&config)["rules"][0]["name"]
        );
        local.plane.close_all().await;
    }

    #[tokio::test]
    async fn summaries_list_every_config_capable_driver() {
        let home = tempdir();
        let (bin, _, _) = fake_driver(home.path(), "fake");
        let (bin2, _, _) = fake_driver(home.path(), "reader");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("fake", true));
        registry.insert(bin2, manifest("reader", false));
        let local = mk_local(registry, home.path());

        let rows = local.agent_config_summaries(None, None).await.unwrap();
        // `reader` lacks AgentConfig — only `fake` reports.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent, "fake");
        let counts = rows[0].counts.as_ref().unwrap();
        assert_eq!(counts.rules, 1);
        assert_eq!(counts.mcp_servers, 1);
        assert_eq!(counts.skills, 0);
        assert!(rows[0].error.is_none());
        local.plane.close_all().await;
    }

    #[tokio::test]
    async fn summaries_reject_unknown_or_non_config_agents() {
        let home = tempdir();
        let (bin, _, _) = fake_driver(home.path(), "reader");
        let mut registry = DriverRegistry::default();
        registry.insert(bin, manifest("reader", false));
        let local = mk_local(registry, home.path());

        let err = local
            .agent_config_summaries(Some("ghost"), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);

        let err = local
            .agent_config_summaries(Some("reader"), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);

        let err = local.agent_config("ghost", None).await.unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);
        let err = local.agent_config("reader", None).await.unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);
        let err = local
            .write_agent_config("reader", None, &AgentConfig::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ControlErrorCode::UnknownAgent);
        local.plane.close_all().await;
    }
}

#[cfg(not(unix))]
mod cfg {}
