#![allow(clippy::unwrap_used, clippy::pedantic)]

//! End-to-end ACP connection test against the mock agent: initialize,
//! session ops, update stream, permission round-trip, close.

use std::sync::Arc;
use std::time::Duration;

use sepia_acp::types::{AgentSpec, PromptPart, SpawnOptions};
use sepia_acp::{AcpSessionUpdate, spawn_agent};

fn mock_spec() -> AgentSpec {
    AgentSpec {
        id: "mock".into(),
        label: "Mock".into(),
        command: vec![env!("CARGO_BIN_EXE_sepia-mock-acp-agent").to_string()],
        env: None,
    }
}

fn options() -> SpawnOptions {
    SpawnOptions {
        cwd: std::env::temp_dir().to_string_lossy().to_string(),
        env: None,
        model: None,
        fallbacks: None,
    }
}

#[tokio::test]
async fn initialize_captures_capabilities() {
    let conn = spawn_agent(&mock_spec(), &options()).await.unwrap();
    let caps = conn.capabilities().await;
    assert!(caps.load_session);
    assert!(caps.prompt_capabilities.image);
    assert!(!caps.prompt_capabilities.audio);
    assert!(caps.session_capabilities.list);
    assert!(caps.session_capabilities.delete);
    assert!(!caps.session_capabilities.fork);
    Arc::new(conn).close().await;
}

#[tokio::test]
async fn session_ops_round_trip() {
    let conn = Arc::new(spawn_agent(&mock_spec(), &options()).await.unwrap());
    let session_id = conn.new_session("/work").await.unwrap();
    assert_eq!(session_id, "mock-session");
    conn.load_session("s1", "/work").await.unwrap();
    conn.delete_session("s1").await.unwrap();
    let sessions = conn.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s1");
    assert!(!sessions[0].locked);
    conn.cancel("s1").await.unwrap();
    conn.close().await;
}

#[tokio::test]
async fn prompt_streams_normalized_updates() {
    let conn = Arc::new(spawn_agent(&mock_spec(), &options()).await.unwrap());
    let mut updates = conn.updates();
    conn.prompt(
        "s1",
        &[PromptPart::Text {
            text: "hi there".into(),
        }],
    )
    .await
    .unwrap();
    let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
        .await
        .unwrap()
        .unwrap();
    let AcpSessionUpdate::AgentMessageChunk { text } = update else {
        panic!("wrong variant {update:?}")
    };
    assert_eq!(text, "echo: hi there");
    conn.close().await;
}

#[tokio::test]
async fn permission_requests_settle_out_of_band() {
    let conn = Arc::new(spawn_agent(&mock_spec(), &options()).await.unwrap());
    let mut permissions = conn.permissions();
    conn.prompt(
        "s1",
        &[PromptPart::Text {
            text: "perm".into(),
        }],
    )
    .await
    .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(5), permissions.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.session_id, "s1");
    assert_eq!(request.tool_call_id.as_deref(), Some("tc-1"));
    assert_eq!(request.title, "Run it");
    assert!(conn.respond_to_permission(&request.request_id, Some("allow")));
    // Unknown ids report false.
    assert!(!conn.respond_to_permission("bogus", Some("allow")));
    conn.close().await;
}
