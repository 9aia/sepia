#![allow(clippy::unwrap_used, clippy::pedantic)]

//! End-to-end driver mechanism test: spawn the memtest driver, probe its
//! manifest, and drive `SessionRepository` ops over stdio JSON-RPC.

use std::path::PathBuf;
use std::sync::Arc;

use sepia_core::{Role, Session, storage::SessionRepository};
use sepia_driver_host::{DriverClient, DriverRegistry, RemoteStore, discover::probe_manifest};
use serde_json::{Value, json};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sepia-driver-memtest"))
}

fn sample_session(id: &str) -> Session {
    Session {
        id: id.into(),
        title: "Sample".into(),
        working_directory: "/work".into(),
        backend_type: "memtest".into(),
        agent_mode: "accept-edits".into(),
        model: "m".into(),
        created_at: 1_700_000_000.0,
        last_activity_at: 1_700_000_100.0,
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: None,
        agent_id: None,
        checkpoints: vec![],
        metadata: Value::Null,
        nodes: vec![sepia_core::MessageNode {
            node_id: 0,
            parent_node_id: None,
            role: Role::User,
            content: "hello".into(),
            blocks: vec![],
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            thinking: None,
            thinking_signature: None,
            usage: None,
            model: None,
            request_id: None,
            finish_reason: None,
            tool_result: None,
            created_at: 1_700_000_000.0,
            metadata: Value::Null,
        }],
        prompt_history: vec![],
    }
}

#[tokio::test]
async fn manifest_probe_returns_the_driver_description() {
    let manifest = probe_manifest(&binary()).await.unwrap();
    assert_eq!(manifest.id, "memtest");
    assert!(
        manifest
            .capabilities
            .contains(&sepia_driver_sdk::Capability::SessionStore)
    );
}

#[tokio::test]
async fn spawn_and_drive_session_ops_over_stdio() {
    let client = DriverClient::spawn(&binary(), &[]).await.unwrap();
    let manifest = sepia_driver_host::refresh_manifest(&client).await.unwrap();
    assert_eq!(manifest.id, "memtest");

    let store = RemoteStore::new(Arc::new(client), "memtest".into());
    assert!(!store.has_session("s1").await.unwrap());
    assert_eq!(store.get_by_id("s1", None).await.unwrap(), None);

    store.save(&sample_session("s1")).await.unwrap();
    store.save(&sample_session("s2")).await.unwrap();

    assert!(store.has_session("s1").await.unwrap());
    let list = store.list().await.unwrap();
    assert_eq!(list.len(), 2);

    let session = store.get_by_id("s1", None).await.unwrap().unwrap();
    assert_eq!(session.title, "Sample");
    assert_eq!(session.nodes.len(), 1);
    assert_eq!(session.nodes[0].role, Role::User);

    let summary = store.summary("s1", None).await.unwrap().unwrap();
    assert!(summary.nodes.is_empty());

    let window = store
        .nodes_window("s1", &Default::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window.total, 1);

    store.delete("s1").await.unwrap();
    assert!(!store.has_session("s1").await.unwrap());
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let client = DriverClient::spawn(&binary(), &[]).await.unwrap();
    let err = client.call("nope.method", json!({})).await.unwrap_err();
    assert_eq!(err.code, sepia_driver_sdk::rpc::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn registry_respawns_a_dead_driver() {
    let mut registry = DriverRegistry::default();
    let manifest = probe_manifest(&binary()).await.unwrap();
    registry.insert(binary(), manifest);
    let entry = registry.with(&sepia_driver_sdk::Capability::SessionStore)[0];

    let first = entry.client().await.unwrap();
    first
        .call("driver.manifest", serde_json::json!({}))
        .await
        .unwrap();
    // Kill the child — stdout closes, is_closed flips.
    first.kill().await;
    // Wait for the reader task to observe the closed stdout.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !first.is_closed() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(first.is_closed());

    // Next client() call spawns a fresh process.
    let second = entry.client().await.unwrap();
    let manifest = second
        .call("driver.manifest", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(manifest["id"], "memtest");
}
