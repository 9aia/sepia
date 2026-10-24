#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `sepia-driver-cursor` end to end — spawn the real binary and drive
//! the ndjson driver wire over pipes; stdin EOF ends it cleanly so an
//! instrumented build flushes its coverage profile.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const METHOD_NOT_FOUND: i64 = -32601;
const SESSION_NOT_FOUND: i64 = -32001;
const CAPABILITY_UNSUPPORTED: i64 = -32004;

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Wire {
    fn spawn(envs: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sepia-driver-cursor"));
        command
            .envs(envs.iter().copied())
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
    let out = Command::new(env!("CARGO_BIN_EXE_sepia-driver-cursor"))
        .arg("--manifest")
        .output()
        .unwrap();
    assert!(out.status.success());
    let manifest: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(manifest["id"], "cursor");
    // Cursor has no agent runtime — sessions are read/history only.
    assert!(manifest["agentCommand"].is_null() || manifest.get("agentCommand").is_none());
}

#[test]
fn serves_the_driver_wire() {
    let tmp = tempfile::tempdir().unwrap();
    let cursor_dir = tmp.path().join(".cursor");
    let mut wire = Wire::spawn(&[("SEPIA_CURSOR_DIR", cursor_dir.to_str().unwrap())]);

    let res = wire.call(1, "driver.manifest", json!({}));
    assert_eq!(res["result"]["id"], "cursor", "{res}");

    // Read-only store — an empty dir lists nothing.
    let res = wire.call(2, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");
    let res = wire.call(3, "session.get", json!({"id": "nope"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");

    // The write surface is gated off — capability errors, not crashes.
    for (id, method, params) in [
        (4, "session.save", json!({})),
        (5, "session.delete", json!({"id": "x"})),
        (6, "session.rename", json!({"id": "x", "title": "t"})),
        (
            7,
            "session.rewind",
            json!({"session": {}, "plan": {}, "truncated": {}}),
        ),
    ] {
        let res = wire.call(id, method, params);
        assert_eq!(
            res["error"]["code"], CAPABILITY_UNSUPPORTED,
            "{method}: {res}"
        );
    }

    // config.* is supported — round-trip through the wire.
    let res = wire.call(
        8,
        "config.write",
        json!({
            "dir": cursor_dir,
            "config": {"rules": [{"name": "wire", "kind": "rule", "body": "b"}]},
        }),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(9, "config.read", json!({"dir": cursor_dir}));
    assert_eq!(res["result"]["rules"][0]["name"], "wire", "{res}");
    let res = wire.call(10, "config.read", Value::Null);
    assert_eq!(res["result"]["rules"][0]["name"], "wire", "{res}");

    let res = wire.call(11, "bogus.method", json!({}));
    assert_eq!(res["error"]["code"], METHOD_NOT_FOUND, "{res}");
}

#[test]
fn default_dir_falls_back_to_home() {
    // No SEPIA_CURSOR_DIR — cursor_dir() takes the HOME arm.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut wire = Wire::spawn(&[("HOME", home.to_str().unwrap())]);
    let res = wire.call(1, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");
}

#[test]
fn an_empty_env_dir_also_falls_back_to_home() {
    // SEPIA_CURSOR_DIR="" — the empty-string guard falls through to HOME.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut wire = Wire::spawn(&[("HOME", home.to_str().unwrap()), ("SEPIA_CURSOR_DIR", "")]);
    let res = wire.call(1, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");
}
