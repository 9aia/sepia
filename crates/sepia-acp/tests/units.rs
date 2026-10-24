#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! In-process coverage for the small pieces: `StderrTail` ring/truncate/
//! remainder behavior, the permission broker's dead/unknown arms, agent
//! registry resolution + model flags, and `spawn_agent`'s early failures.

use std::sync::Arc;
use std::time::Duration;

use sepia_acp::registry::{model_args, resolve_agent};
use sepia_acp::spawn_agent;
use sepia_acp::stderr::StderrTail;
use sepia_acp::types::{AgentSpec, SpawnOptions};
use serde_json::{Value, json};

// ---------- StderrTail ---------------------------------------------------------

#[test]
fn stderr_tail_buffers_partials_rings_and_truncates() {
    let tail = StderrTail::new("mock", false);
    // A partial line waits in the remainder until its newline lands.
    tail.push("partial");
    assert!(tail.recent().is_empty());
    tail.push(" line\ndone\n");
    let lines = tail.recent();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("partial line"), "{lines:?}");
    assert!(lines[1].contains("done"), "{lines:?}");

    // Lines over 500 chars truncate.
    let long = "x".repeat(700);
    tail.push(&format!("{long}\n"));
    let last = tail.recent().pop().unwrap();
    assert!(last.len() < 600, "{last} not truncated");

    // The ring keeps only the newest 100.
    for i in 0..150 {
        tail.push(&format!("n{i}\n"));
    }
    let lines = tail.recent();
    assert_eq!(lines.len(), 100);
    assert!(lines.last().unwrap().contains("n149"));
}

#[test]
fn stderr_tail_debug_echoes_to_stderr() {
    let tail = StderrTail::new("mock", true);
    tail.push("visible\n");
    assert_eq!(tail.recent().len(), 1);
}

// ---------- PermissionBroker ---------------------------------------------------

#[tokio::test]
async fn broker_dead_lines_and_unknown_ids() {
    use sepia_acp::broker::{PermissionBroker, outcome_cancelled, outcome_selected};

    assert_eq!(outcome_selected("a")["outcome"]["optionId"], "a");
    assert_eq!(outcome_cancelled()["outcome"]["outcome"], "cancelled");

    let broker = PermissionBroker::new();
    // Unknown id responds false.
    assert!(!broker.respond("ghost", Some("x")));

    let (req, rx) = broker.begin(&json!({
        "sessionId": "s1",
        "toolCall": {"toolCallId": "t", "title": "go"},
        "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"}],
    }));
    assert!(!req.request_id.is_empty());
    assert!(broker.respond(&req.request_id, None)); // cancelled arm
    assert!(rx.await.unwrap().is_ok());

    // fail_all settles pending and poisons later begins.
    let broker = PermissionBroker::new();
    let (req2, rx2) = broker.begin(&json!({"sessionId": "s"}));
    broker.fail_all("dead");
    assert!(rx2.await.unwrap().is_err());
    let (_req3, rx3) = broker.begin(&json!({"sessionId": "s"}));
    assert!(rx3.await.unwrap().is_err());
    let _ = req2;
}

// ---------- registry -------------------------------------------------------------

#[test]
fn resolve_agent_prefers_overrides_then_builtins() {
    let custom = AgentSpec {
        id: "devin".into(),
        label: "Custom".into(),
        command: vec!["x".into()],
        env: None,
    };
    let found = resolve_agent("devin", &[custom.clone()]).unwrap();
    assert_eq!(found.label, "Custom");
    // Builtin fallback.
    assert_eq!(resolve_agent("cline", &[]).unwrap().label, "Cline");
    // Unknown → error listing the known ids.
    let err = resolve_agent("ghost", &[custom]).unwrap_err();
    assert!(err.contains("ghost") && err.contains("devin"), "{err}");
    let err = resolve_agent("ghost", &[]).unwrap_err();
    assert!(err.contains("devin"), "{err}");
}

#[test]
fn model_args_per_agent_shape() {
    // cline takes -m, no fallback flag.
    assert_eq!(
        model_args("cline", Some("m"), Some(&["f".into()])),
        vec!["-m", "m"]
    );
    assert!(model_args("cline", None, None).is_empty());
    // claude ignores both.
    assert!(model_args("claude", Some("m"), Some(&["f".into()])).is_empty());
    // devin gets --model and --refusal-fallback.
    assert_eq!(
        model_args("devin", Some("m"), Some(&["a".into(), "b".into()])),
        vec!["--model", "m", "--refusal-fallback", "a,b"]
    );
    assert_eq!(model_args("devin", None, Some(&[])), Vec::<String>::new());
    // Other agents get --model only.
    assert_eq!(model_args("other", Some("m"), None), vec!["--model", "m"]);
    assert!(model_args("other", None, Some(&["f".into()])).is_empty());
}

// ---------- spawn_agent edges -----------------------------------------------------

fn spec(command: Vec<String>) -> AgentSpec {
    AgentSpec {
        id: "x".into(),
        label: "X".into(),
        command,
        env: None,
    }
}

fn options() -> SpawnOptions {
    SpawnOptions {
        cwd: String::new(), // exercises the "." fallback
        env: Some([("K".to_string(), "V".to_string())].into_iter().collect()),
        model: Some("m".into()),
        fallbacks: Some(vec!["f".into()]),
    }
}

#[tokio::test]
async fn spawn_fails_on_empty_command_and_missing_binary() {
    let err = spawn_agent(&spec(vec![]), &options()).await.err().unwrap();
    assert!(err.message.contains("no command"), "{err}");

    let err = spawn_agent(&spec(vec!["/nonexistent-sepia-agent".into()]), &options())
        .await
        .err()
        .unwrap();
    assert!(err.message.contains("failed to spawn"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn call_after_agent_death_fails_fast() {
    // The agent answers initialize, then exits while a backgrounded
    // `sleep` keeps its stdout pipe open — the exit watcher (not the
    // reader's EOF arm) must record the death, so calls land on the
    // exit_error short-circuit.
    let conn = spawn_agent(
        &spec(vec![
            "sh".into(),
            "-c".into(),
            "printf '%s\\n' \
             '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}'; \
             sleep 10 & exit 0"
                .into(),
        ]),
        &options(),
    )
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match conn.list_sessions().await {
            Err(e) if e.message.contains("exited") => break,
            _ if std::time::Instant::now() > deadline => {
                panic!("agent never marked dead")
            }
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    Arc::new(conn).close().await;
}

#[test]
fn mock_agent_tolerates_garbage_and_unknown_methods() {
    // Drive the mock directly: blank lines, non-JSON lines, and an
    // unknown request all get tolerated or answered.
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_sepia-mock-acp-agent"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "not json at all").unwrap();
        writeln!(stdin).unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"session/cancel","params":{}})
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"bogus/method","params":{}})
        )
        .unwrap();
    }
    use std::io::BufRead as _;
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let res: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(res["error"]["code"], -32601, "{res}");
    drop(reader);
    drop(child.stdin.take());
    child.wait().unwrap();
}

#[tokio::test]
async fn updates_permission_and_stderr_paths() {
    let conn = spawn_agent(
        &spec(vec![env!("CARGO_BIN_EXE_sepia-mock-acp-agent").to_string()]),
        &SpawnOptions {
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            env: None,
            model: None,
            fallbacks: None,
        },
    )
    .await
    .unwrap();
    // Subscription handles + the stderr tail accessor.
    let _updates = conn.updates();
    let _permissions = conn.permissions();
    let _ = conn.recent_stderr();
    assert!(!conn.respond_to_permission("unknown", None));
    Arc::new(conn).close().await;
}
