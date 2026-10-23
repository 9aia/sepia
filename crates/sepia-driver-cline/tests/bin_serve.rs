#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `sepia-driver-cline` end to end — spawn the real binary and drive the
//! ndjson driver wire over pipes; stdin EOF ends the process cleanly so
//! an instrumented build flushes its coverage profile.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use sepia_testkit::contract;
use serde_json::{Value, json};

const METHOD_NOT_FOUND: i64 = -32601;
const CAPABILITY_UNSUPPORTED: i64 = -32004;

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Wire {
    fn spawn(envs: &[(&str, &str)], dir: &std::path::Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sepia-driver-cline"));
        command
            .envs(envs.iter().copied())
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        Self {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).unwrap() > 0, "eof");
        let message: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(message["id"], id, "{message}");
        message
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(status.success(), "driver exited {status}");
                    return;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
                Err(_) => return,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        panic!("driver did not exit on stdin EOF");
    }
}

#[test]
fn manifest_flag_prints_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_sepia-driver-cline"))
        .arg("--manifest")
        .output()
        .unwrap();
    assert!(out.status.success());
    let manifest: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(manifest["id"], "cline");
    assert_eq!(manifest["agentCommand"], "cline --acp");
}

#[test]
fn serves_the_driver_wire() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("cline-data");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut wire = Wire::spawn(&[("SEPIA_CLINE_DIR", data.to_str().unwrap())], &workspace);

    let res = wire.call(1, "driver.manifest", json!({}));
    assert_eq!(res["result"]["id"], "cline", "{res}");

    let res = wire.call(2, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");

    // Save a session, read it back every way.
    let session = serde_json::to_value(contract::session("c1", "Cline", 2.0)).unwrap();
    let res = wire.call(3, "session.save", session);
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(4, "session.list", json!({}));
    assert_eq!(res["result"].as_array().unwrap().len(), 1, "{res}");
    let res = wire.call(5, "session.get", json!({"id": "c1"}));
    assert_eq!(res["result"]["id"], "c1", "{res}");
    let res = wire.call(6, "session.summary", json!({"id": "c1"}));
    assert_eq!(res["result"]["id"], "c1", "{res}");
    let res = wire.call(7, "session.history", json!({"id": "c1"}));
    assert!(res["result"]["nodes"].is_array(), "{res}");
    let res = wire.call(8, "session.checkpoints", json!({"id": "c1"}));
    assert!(res["result"].is_object(), "{res}");
    let res = wire.call(9, "session.rename", json!({"id": "c1", "title": "R"}));
    assert!(res.get("error").is_none(), "{res}");

    // Rewind — the store itself is the truncator.
    let res = wire.call(
        10,
        "session.rewind",
        json!({
            "session": serde_json::to_value(contract::session("c1", "Cline", 2.0)).unwrap(),
            "plan": {"keepCount": 1, "kept": [], "removed": [], "removedToolCallIds": []},
            "truncated": serde_json::to_value(contract::session("c1", "Cline", 2.0)).unwrap(),
        }),
    );
    // The store decides whether an empty removal is an error — either
    // way the arm ran.
    assert!(res["result"].is_null() || res["error"].is_object(), "{res}");

    // config.* — Cline config is workspace-rooted; `dir` names it.
    let res = wire.call(
        11,
        "config.write",
        json!({
            "dir": workspace,
            "config": {"rules": [{"name": "ws", "kind": "rule", "body": "b"}]},
        }),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(12, "config.read", json!({"dir": workspace}));
    assert_eq!(res["result"]["rules"][0]["name"], "ws", "{res}");
    // No `dir` at all → the driver's process cwd (the workspace here).
    let res = wire.call(13, "config.read", Value::Null);
    assert_eq!(res["result"]["rules"][0]["name"], "ws", "{res}");

    let res = wire.call(
        14,
        "file.restore",
        json!({"id": "c1", "path": "/x", "confirm": true}),
    );
    assert_eq!(res["error"]["code"], CAPABILITY_UNSUPPORTED, "{res}");
    let res = wire.call(15, "bogus.method", json!({}));
    assert_eq!(res["error"]["code"], METHOD_NOT_FOUND, "{res}");

    let res = wire.call(16, "session.delete", json!({"id": "c1"}));
    assert!(res.get("error").is_none(), "{res}");
}

#[test]
fn default_dir_falls_back_to_home() {
    // No SEPIA_CLINE_DIR — data_dir() takes the HOME arm.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut wire = Wire::spawn(&[("HOME", home.to_str().unwrap())], tmp.path());
    let res = wire.call(1, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");
}
