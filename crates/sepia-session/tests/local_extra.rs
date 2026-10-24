#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `local.rs` tail coverage: `build_with` discovery filtering, per-agent
//! capability gating, driver-failure degradation in config/summary/
//! import/rewind paths.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use sepia_core::Role;
use sepia_core::agent_config::AgentConfig;
use sepia_driver::{Capability, DRIVER_PROTOCOL, DriverManifest};
use sepia_host::DriverRegistry;
use sepia_session::local::{ImportOverrides, Paths, build_with};
use sepia_session::{ControlErrorCode, ControlPlaneOptions};
use serde_json::json;

/// A driver that answers every request with an RPC error — exercises the
/// `map_err` conversion arms in `agent_config`/`write_agent_config`/
/// `import_session`/`DriverRewinder`.
fn failing_driver(dir: &Path, name: &str) -> PathBuf {
    let bin = dir.join(format!("sepia-driver-{name}"));
    std::fs::write(
        &bin,
        "#!/bin/sh\n\
         while IFS= read -r line; do\n\
         \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
         \tprintf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32000,\"message\":\"driver down\"}}\\n' \"$id\"\n\
         done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn manifest(id: &str, caps: &[Capability]) -> DriverManifest {
    DriverManifest {
        id: id.into(),
        label: id.into(),
        version: "0.0.1".into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: caps.iter().cloned().collect(),
        agent_command: Some("/bin/true".into()),
        config_schema: json!({}),
        backend_type: Some(id.into()),
    }
}

fn paths(home: &Path) -> Paths {
    Paths::new(home.to_path_buf())
}

#[tokio::test]
async fn build_with_injects_warnings_and_filtered_agents() {
    let home = tempdir();
    let mut registry = DriverRegistry::default();
    // A driver claiming the built-in `devin` id — its agent_command
    // resolves (/bin/true), so the devin spec survives the filter.
    registry.insert(
        home.path().join("missing-bin"),
        manifest("devin", &[Capability::SessionStore]),
    );
    // A non-built-in id → merged store member (spawn will fail on use).
    registry.insert(
        home.path().join("missing2"),
        manifest("extra", &[Capability::SessionStore, Capability::Rewind]),
    );
    let local = build_with(
        &paths(home.path()),
        ControlPlaneOptions::default(),
        registry,
        vec!["fake problem: driver x".into()],
    )
    .await
    .unwrap();
    // The rewinder loop logged + skipped the unspawning `extra` driver.
    let agents = local.plane.list_agents().await;
    assert!(agents.iter().any(|a| a.id == "devin"));
}

#[tokio::test]
async fn capable_store_and_config_error_arms() {
    let home = tempdir();
    let mut registry = DriverRegistry::default();
    // AgentConfig-capable but errors on every call.
    registry.insert(
        failing_driver(home.path(), "cfg"),
        manifest("cfg", &[Capability::AgentConfig]),
    );
    // SessionStore-only → lacks the capability.
    registry.insert(
        failing_driver(home.path(), "ro"),
        manifest("ro", &[Capability::SessionStore]),
    );
    // Binary missing → spawn fails → internal.
    registry.insert(
        home.path().join("does-not-exist"),
        manifest("gone", &[Capability::AgentConfig, Capability::SessionWrite]),
    );
    let local = build_with(
        &paths(home.path()),
        ControlPlaneOptions::default(),
        registry,
        vec![],
    )
    .await
    .unwrap();

    // unknown agent → UnknownAgent.
    let err = local.agent_config("ghost", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    // agent lacking AgentConfig → UnknownAgent.
    let err = local.agent_config("ro", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    // unspawnable driver → Internal.
    let err = local.agent_config("gone", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);
    // driver answers an error → internal map_err arm.
    let err = local.agent_config("cfg", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);

    // write_agent_config: same three-arm surface plus the wire error.
    let config = AgentConfig::default();
    assert_eq!(
        local
            .write_agent_config("ghost", None, &config)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::UnknownAgent
    );
    assert_eq!(
        local
            .write_agent_config("ro", None, &config)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::UnknownAgent
    );
    assert_eq!(
        local
            .write_agent_config("gone", None, &config)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Internal
    );
    assert_eq!(
        local
            .write_agent_config("cfg", None, &config)
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::Internal
    );
}

#[tokio::test]
async fn agent_config_summaries_degrade_per_agent() {
    let home = tempdir();
    let mut registry = DriverRegistry::default();
    registry.insert(
        failing_driver(home.path(), "bad"),
        manifest("bad", &[Capability::AgentConfig]),
    );
    registry.insert(
        home.path().join("missing"),
        manifest("gone", &[Capability::AgentConfig]),
    );
    // A driver with no AgentConfig → skipped by the summary filter.
    registry.insert(
        failing_driver(home.path(), "store"),
        manifest("store", &[Capability::SessionStore]),
    );
    let local = build_with(
        &paths(home.path()),
        ControlPlaneOptions::default(),
        registry,
        vec![],
    )
    .await
    .unwrap();
    let summaries = local.agent_config_summaries(None, None).await.unwrap();
    let by_id: std::collections::HashMap<&str, &sepia_session::local::AgentConfigSummary> =
        summaries.iter().map(|s| (s.agent.as_str(), s)).collect();
    assert!(by_id["bad"].error.is_some());
    assert!(by_id["gone"].error.is_some());
    assert!(!by_id.contains_key("store"));
    // Naming one agent narrows the summary to it.
    let one = local
        .agent_config_summaries(Some("bad"), None)
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
}

#[tokio::test]
async fn import_save_and_override_paths() {
    let home = tempdir();
    let mut registry = DriverRegistry::default();
    // A write-capable driver that fails saves.
    registry.insert(
        failing_driver(home.path(), "sink"),
        manifest(
            "sink",
            &[Capability::SessionStore, Capability::SessionWrite],
        ),
    );
    let local = build_with(
        &paths(home.path()),
        ControlPlaneOptions::default(),
        registry,
        vec![],
    )
    .await
    .unwrap();
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    // unknown agent.
    assert_eq!(
        local
            .import_session("ghost", &session, &ImportOverrides::default())
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::UnknownAgent
    );
    // save failure → internal.
    let err = local
        .import_session("sink", &session, &ImportOverrides::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);
    // Override fields are applied before save (still fails, but exercises
    // the override arms).
    let err = local
        .import_session(
            "sink",
            &session,
            &ImportOverrides {
                id: Some("renamed".into()),
                cwd: Some("/elsewhere".into()),
                title: Some("New title".into()),
                model: Some("m".into()),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);
}

/// A driver that serves `session.get`/`session.list` and fails
/// `session.rewind` — the `DriverRewinder::truncate` map_err path needs a
/// session to actually reach the wire call.
fn rewinder_driver(dir: &Path) -> PathBuf {
    let bin = dir.join("sepia-driver-rw");
    std::fs::write(
        &bin,
        "#!/bin/sh\n\
         while IFS= read -r line; do\n\
         \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
         \tcase \"$line\" in\n\
         \t\t*'\"method\":\"session.get\"'*) result='{\"id\":\"s1\",\"title\":\"t\",\"workingDirectory\":\"/w\",\"model\":\"m\",\"createdAt\":1,\"lastActivityAt\":2,\"mainChainId\":1,\"nodes\":[{\"nodeId\":0,\"role\":\"user\",\"content\":\"q\",\"createdAt\":1},{\"nodeId\":1,\"role\":\"assistant\",\"content\":\"a\",\"createdAt\":2}]}' ;;\n\
         \t\t*'\"method\":\"session.list\"'*) result='[]' ;;\n\
         \t\t*) printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{\"code\":-32000,\"message\":\"cut failed\"}}\\n' \"$id\"; continue ;;\n\
         \tesac\n\
         \tprintf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}\\n' \"$id\" \"$result\"\n\
         done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

#[tokio::test]
async fn rewinder_truncate_surfaces_driver_errors() {
    let home = tempdir();
    let mut registry = DriverRegistry::default();
    // Rewind-capable, id "devin" — `agent_for_backend` maps the unknown
    // backend_type to "devin" so the rewinder lookup hits.
    registry.insert(
        rewinder_driver(home.path()),
        manifest("devin", &[Capability::SessionStore, Capability::Rewind]),
    );
    let local = build_with(
        &paths(home.path()),
        ControlPlaneOptions::default(),
        registry,
        vec![],
    )
    .await
    .unwrap();
    let err = local
        .plane
        .rewind(
            "s1",
            &sepia_session::RewindRequest {
                confirm: true,
                node_id: None,
                turns: Some(1),
                checkpoint: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);
}
