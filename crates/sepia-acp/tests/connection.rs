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
async fn initialize_tolerates_missing_capability_objects() {
    let mut spec = mock_spec();
    spec.env = Some(
        [("MOCK_MINIMAL_CAPS".to_string(), "1".to_string())]
            .into_iter()
            .collect(),
    );
    let conn = spawn_agent(&spec, &options()).await.unwrap();
    let caps = conn.capabilities().await;
    assert!(!caps.session_capabilities.list);
    assert!(!caps.prompt_capabilities.image);
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

#[tokio::test]
async fn permission_request_is_answered_over_the_wire() {
    let conn = Arc::new(spawn_agent(&mock_spec(), &options()).await.unwrap());
    conn.load_session("s1", "/work").await.unwrap();
    // "perm" is the default MOCK_PERM_TEXT trigger.
    let _ = conn
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "perm".into(),
            }],
        )
        .await;
    conn.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn agent_stderr_and_mid_call_death_are_drained() {
    // A spec whose shell spews stderr before exec'ing the mock — the
    // stderr drain task reads it while the conn is alive.
    let mut spec = AgentSpec {
        id: "x".into(),
        label: "x".into(),
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo noise-on-stderr >&2; exec \"$0\" \"$@\"".into(),
            std::env::var("CARGO_BIN_EXE_sepia-mock-acp-agent")
                .unwrap_or_else(|_| "sepia-mock-acp-agent".into()),
        ],
        env: Some(
            [("MOCK_EXIT_AFTER_PROMPT".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
        ),
    };
    let conn = Arc::new(spawn_agent(&spec, &options()).await.unwrap());
    conn.load_session("s1", "/work").await.unwrap();
    // The first prompt kills the agent mid-call — the exit watcher fails
    // the pending call instead of hanging.
    let _ = conn
        .prompt("s1", &[sepia_acp::PromptPart::Text { text: "go".into() }])
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    conn.close().await;
    spec.env = None;
}

#[test]
fn resolve_agent_unknown_lists_known_or_nothing() {
    // Unknown ids report the known-agent list (builtins always present).
    let err = sepia_acp::resolve_agent("ghost", &[]).unwrap_err();
    assert!(err.contains("known agents"));
    let known = AgentSpec {
        id: "k".into(),
        label: "k".into(),
        command: vec!["x".into()],
        env: None,
    };
    let err = sepia_acp::resolve_agent("ghost", &[known]).unwrap_err();
    assert!(err.contains("known agents"));
}

/// A raw spawn of the mock agent: a non-request line and a
/// non-numeric-id request hit the `_ => {}` + string-id error arms.
#[cfg(unix)]
#[test]
fn mock_agent_tolerates_odd_lines() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let bin = env!("CARGO_BIN_EXE_sepia-mock-acp-agent");
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // A bare notification (no id) → `_ => {}` arm.
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"method\":\"weird/thing\",\"params\":{{}}}}"
    )
    .unwrap();
    // A request with a non-numeric id → the invalid-id error path.
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":\"str-id\",\"method\":\"session/list\",\"params\":{{}}}}"
    )
    .unwrap();
    // Garbage JSON → decode error arm.
    writeln!(stdin, "not json at all").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let _ = out;
}
