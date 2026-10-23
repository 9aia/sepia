#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `config.read`/`config.write` dispatch on `StoreDriver` — capability
//! gating, param parsing, `spawn_blocking` hand-off, and the
//! `{actions}` result shape. The backend is a fake `AgentConfigStore`;
//! no filesystem is touched.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sepia_core::agent_config::{
    AgentConfig, ConfigRule, ConfigWriteAction, RuleKind, WriteActionKind,
};
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use sepia_core::{Session, StorageError};
use sepia_driver::manifest::{Capability, DRIVER_PROTOCOL, DriverManifest};
use sepia_driver::rpc;
use sepia_driver::{AgentConfigStore, Driver, StoreDriver, methods};
use serde_json::{Value, json};

struct NoopStore;

#[async_trait::async_trait]
impl SessionRepository for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), StorageError> {
        Ok(())
    }
    async fn get_by_id(
        &self,
        _id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        Ok(None)
    }
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        Ok(Vec::new())
    }
    async fn nodes_window(
        &self,
        _id: &str,
        _options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        Ok(None)
    }
    async fn delete(&self, _id: &str) -> Result<(), StorageError> {
        Ok(())
    }
    async fn has_session(&self, _id: &str) -> Result<bool, StorageError> {
        Ok(false)
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

/// Records the `dir` each call resolved and answers canned data.
#[derive(Default)]
struct FakeConfig {
    seen: Mutex<Vec<Option<PathBuf>>>,
}

impl AgentConfigStore for FakeConfig {
    fn read_config(&self, dir: Option<PathBuf>) -> Result<AgentConfig, StorageError> {
        self.seen.lock().unwrap().push(dir);
        Ok(AgentConfig {
            rules: vec![rule("house-rules")],
            mcp_servers: json!({"docs": {"command": "mcp-docs"}})
                .as_object()
                .unwrap()
                .clone(),
            ..AgentConfig::default()
        })
    }

    fn write_config(
        &self,
        dir: Option<PathBuf>,
        config: &AgentConfig,
    ) -> Result<Vec<ConfigWriteAction>, StorageError> {
        self.seen.lock().unwrap().push(dir);
        Ok(vec![ConfigWriteAction {
            path: format!("/out/{}.md", config.rules.len()),
            action: WriteActionKind::Wrote,
            detail: None,
        }])
    }
}

fn manifest(capabilities: &[Capability]) -> DriverManifest {
    DriverManifest {
        id: "fake".into(),
        label: "Fake".into(),
        version: "0.0.1".into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: capabilities.iter().cloned().collect(),
        agent_command: None,
        config_schema: json!({}),
        backend_type: Some("fake".into()),
    }
}

#[tokio::test]
async fn config_read_serves_the_ir_and_passes_dir() {
    let config = Arc::new(FakeConfig::default());
    let driver = StoreDriver::new(
        manifest(&[Capability::SessionStore, Capability::AgentConfig]),
        Arc::new(NoopStore),
    )
    .with_config(config.clone());

    let result = driver
        .handle(methods::CONFIG_READ, json!({"dir": "/work/repo"}))
        .await
        .unwrap();

    assert_eq!(result["rules"][0]["name"], "house-rules");
    assert_eq!(result["mcpServers"]["docs"]["command"], "mcp-docs");
    assert_eq!(
        config.seen.lock().unwrap().as_slice(),
        &[Some(PathBuf::from("/work/repo"))]
    );
}

#[tokio::test]
async fn config_write_decodes_the_ir_and_returns_actions() {
    let config = Arc::new(FakeConfig::default());
    let driver = StoreDriver::new(
        manifest(&[Capability::SessionStore, Capability::AgentConfig]),
        Arc::new(NoopStore),
    )
    .with_config(config.clone());

    let payload = sepia_core::agent_config::config_to_json(&AgentConfig {
        rules: vec![rule("a"), rule("b")],
        ..AgentConfig::default()
    });
    let result = driver
        .handle(
            methods::CONFIG_WRITE,
            json!({"config": payload, "dir": "/work/repo"}),
        )
        .await
        .unwrap();

    assert_eq!(result["actions"][0]["path"], "/out/2.md");
    assert_eq!(result["actions"][0]["action"], "wrote");
}

#[tokio::test]
async fn config_methods_require_the_capability_and_a_backend() {
    // No AgentConfig capability → capability_unsupported.
    let driver = StoreDriver::new(manifest(&[Capability::SessionStore]), Arc::new(NoopStore));
    let err = driver
        .handle(methods::CONFIG_READ, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc::CAPABILITY_UNSUPPORTED);

    // Capability advertised but no backend attached → same gate.
    let driver = StoreDriver::new(
        manifest(&[Capability::SessionStore, Capability::AgentConfig]),
        Arc::new(NoopStore),
    );
    let err = driver
        .handle(methods::CONFIG_WRITE, json!({"config": {}}))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc::CAPABILITY_UNSUPPORTED);
}

#[tokio::test]
async fn config_write_rejects_a_malformed_payload() {
    let driver = StoreDriver::new(
        manifest(&[Capability::SessionStore, Capability::AgentConfig]),
        Arc::new(NoopStore),
    )
    .with_config(Arc::new(FakeConfig::default()));

    // `rules` must be an array of objects — a string fails the IR decode.
    let err = driver
        .handle(methods::CONFIG_WRITE, json!({"config": {"rules": "oops"}}))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc::INVALID_PARAMS);

    // `config` missing entirely → param shape error.
    let err = driver
        .handle(methods::CONFIG_WRITE, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, rpc::INVALID_PARAMS);
}

#[tokio::test]
async fn config_read_default_dir_is_none_to_the_backend() {
    let config = Arc::new(FakeConfig::default());
    let driver = StoreDriver::new(
        manifest(&[Capability::SessionStore, Capability::AgentConfig]),
        Arc::new(NoopStore),
    )
    .with_config(config.clone());

    driver
        .handle(methods::CONFIG_READ, Value::Null)
        .await
        .unwrap();
    assert_eq!(config.seen.lock().unwrap().as_slice(), &[None]);
}
