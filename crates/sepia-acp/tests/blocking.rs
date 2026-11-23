#![allow(clippy::unwrap_used, clippy::pedantic)]

//! End-to-end coverage for the synchronous facade: spawn →
//! initialize → session/new → prompt (streamed text, auto-approved
//! permissions) → cancel → close, all against the mock ACP agent —
//! the same fixture the async connection tests drive.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sepia_acp::blocking::{Client, ClientOptions};
use sepia_acp::types::WireDebug;

fn mock_bin() -> String {
    env!("CARGO_BIN_EXE_sepia-mock-acp-agent").to_string()
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sepia-blocking-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn env_of(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
    Some(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    )
}

fn connect(env: Option<BTreeMap<String, String>>, options: ClientOptions) -> Client {
    let mut options = options;
    options.env = env;
    options.client_name = Some("blocking-test".into());
    Client::connect_shell(&mock_bin(), &tmpdir("cwd"), &options).unwrap()
}

#[test]
fn session_prompt_streams_message_text_only() {
    // user_message_chunk and agent_thought_chunk updates must never
    // reach the collected text — only agent_message_chunk counts.
    let client = connect(
        env_of(&[("MOCK_THOUGHT_CHUNK", "1"), ("MOCK_USER_CHUNK", "1")]),
        ClientOptions::default(),
    );
    let sid = client.new_session().unwrap();
    assert_eq!(sid, "mock-session");
    let (result, text) = client.prompt_text(&sid, "hi there").unwrap();
    assert_eq!(result.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(text, "echo: hi there");
    client.cancel(&sid).unwrap();
}

#[test]
fn permission_requests_are_auto_approved() {
    // "perm" is the mock's MOCK_PERM_TEXT trigger — the client must
    // answer session/request_permission out of band or the turn
    // stalls.
    let client = connect(None, ClientOptions::default());
    let sid = client.new_session().unwrap();
    let (_result, text) = client.prompt_text(&sid, "perm").unwrap();
    assert!(text.contains("echo: perm"), "{text}");
}

#[test]
fn call_timeout_surfaces_as_error() {
    let client = connect(
        env_of(&[("MOCK_PROMPT_SLOW_MS", "5000")]),
        ClientOptions {
            call_timeout: Some(Duration::from_millis(150)),
            ..Default::default()
        },
    );
    let sid = client.new_session().unwrap();
    let err = client.prompt_text(&sid, "hi").unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[test]
fn agent_death_mid_prompt_fails_not_hangs() {
    let client = connect(
        env_of(&[("MOCK_EXIT_AFTER_PROMPT", "1")]),
        ClientOptions {
            call_timeout: Some(Duration::from_secs(10)),
            ..Default::default()
        },
    );
    let sid = client.new_session().unwrap();
    assert!(client.prompt_text(&sid, "go").is_err());
}

#[test]
fn spawn_failure_is_an_error() {
    let options = ClientOptions {
        client_name: Some("blocking-test".into()),
        ..Default::default()
    };
    let err = Client::connect_shell("definitely-not-an-agent-xyz", Path::new("."), &options)
        .err()
        .unwrap();
    assert!(!err.is_empty());
}

#[test]
fn wire_debug_records_the_transcript() {
    let log = tmpdir("wire").join("acp.log");
    let client = connect(
        None,
        ClientOptions {
            wire_debug: Some(WireDebug::File(log.clone())),
            ..Default::default()
        },
    );
    let sid = client.new_session().unwrap();
    let _ = client.prompt_text(&sid, "hi").unwrap();
    drop(client);
    let transcript = std::fs::read_to_string(&log).unwrap();
    assert!(transcript.contains("session/prompt"), "{transcript}");
    assert!(transcript.contains("agent_message_chunk"), "{transcript}");
    assert!(transcript.contains('>'), "{transcript}");
    assert!(transcript.contains('<'), "{transcript}");
}

#[test]
fn fs_capability_serves_reads_under_root() {
    // fs_root advertises {"fs":{"readTextFile":true}} at initialize and
    // wires the read handler — the mock doesn't issue fs requests, so
    // this covers the option plumbing end to end (the handler itself
    // is unit-tested in blocking.rs).
    let root = tmpdir("fsroot");
    std::fs::write(root.join("note.md"), "data").unwrap();
    let client = connect(
        None,
        ClientOptions {
            fs_root: Some(root),
            ..Default::default()
        },
    );
    let sid = client.new_session().unwrap();
    let (_r, text) = client.prompt_text(&sid, "hi").unwrap();
    assert!(text.contains("echo: hi"), "{text}");
}
