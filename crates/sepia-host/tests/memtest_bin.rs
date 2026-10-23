#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `sepia-driver-memtest` driven over raw ndjson — every `session.*`
//! method plus the wire-level edge frames (blank lines, notifications,
//! responses, garbage). Stdin EOF ends the process cleanly so an
//! instrumented build flushes its coverage profile.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const SESSION_NOT_FOUND: i64 = -32001;
const CAPABILITY_UNSUPPORTED: i64 = -32004;

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Wire {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sepia-driver-memtest"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string());
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

fn session_json(id: &str, nodes: usize) -> Value {
    let node = |i: i64, tool_calls: Value| {
        json!({
            "nodeId": i,
            "role": "user",
            "content": format!("node {i}"),
            "blocks": [],
            "toolCalls": tool_calls,
            "createdAt": 1700000000.0 + i as f64,
        })
    };
    let nodes: Vec<Value> = (0..nodes as i64)
        .map(|i| {
            node(
                i,
                if i == 0 {
                    json!([])
                } else {
                    json!([{
                        "id": format!("tc{i}"),
                        "name": "run",
                        "arguments": {},
                        "index": 0,
                        "kind": "function",
                    }])
                },
            )
        })
        .collect();
    json!({
        "id": id,
        "title": "Mem",
        "workingDirectory": "/work",
        "backendType": "memtest",
        "agentMode": "accept-edits",
        "model": "m",
        "createdAt": 1700000000.0,
        "lastActivityAt": 1700000001.0,
        "mainChainId": 0,
        "shellLastSeenIndex": 0,
        "cogsJson": "[]",
        "workspaceDirs": "[]",
        "hidden": 0,
        "checkpoints": [],
        "metadata": null,
        "nodes": nodes,
        "promptHistory": [],
    })
}

#[test]
fn manifest_flag_prints_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_sepia-driver-memtest"))
        .arg("--manifest")
        .output()
        .unwrap();
    assert!(out.status.success());
    let manifest: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(manifest["id"], "memtest");
}

#[test]
fn serves_the_whole_session_surface() {
    let mut wire = Wire::spawn();

    // Wire-level edges first: garbage, non-object, blank, notification,
    // a stray response — then real traffic.
    wire.send("hello");
    {
        let mut line = String::new();
        wire.stdout.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
            PARSE_ERROR
        );
    }
    wire.send("\"a string\"");
    {
        let mut line = String::new();
        wire.stdout.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
            INVALID_REQUEST
        );
    }
    wire.send("{\"jsonrpc\":\"2.0\"}"); // neither id nor method
    {
        let mut line = String::new();
        wire.stdout.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
            INVALID_REQUEST
        );
    }
    wire.send("");
    wire.send(&json!({"jsonrpc":"2.0","method":"note","params":{"a":1}}).to_string());
    wire.send(&json!({"jsonrpc":"2.0","id":42,"result":{"x":1}}).to_string());

    let res = wire.call(1, "driver.manifest", json!({}));
    assert_eq!(res["result"]["backendType"], "memtest", "{res}");

    // Write, then read back every shape.
    let session = session_json("s1", 3);
    let res = wire.call(2, "session.save", session.clone());
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(3, "session.list", json!({}));
    assert_eq!(res["result"].as_array().unwrap().len(), 1);
    let res = wire.call(4, "session.get", json!({"id": "s1"}));
    assert_eq!(res["result"]["title"], "Mem", "{res}");
    let res = wire.call(5, "session.summary", json!({"id": "s1"}));
    assert_eq!(res["result"]["id"], "s1", "{res}");

    // nodes_window: limit/before/agentId all ride the window options.
    let res = wire.call(
        6,
        "session.history",
        json!({"id": "s1", "limit": 2, "before": 3, "agentId": "memtest"}),
    );
    let window = &res["result"];
    assert_eq!(window["total"], 3, "{window}");
    assert_eq!(window["nodes"].as_array().unwrap().len(), 2, "{window}");
    assert_eq!(window["start"], 1, "{window}");
    // The tool-call carrying node lands in toolCallNodes.
    assert_eq!(
        window["toolCallNodes"].as_array().unwrap().len(),
        2,
        "{window}"
    );
    // Negative `before` clamps to zero rather than panicking.
    let res = wire.call(7, "session.history", json!({"id": "s1", "before": -5}));
    assert_eq!(res["result"]["nodes"].as_array().unwrap().len(), 0, "{res}");
    let res = wire.call(8, "session.history", json!({"id": "nope"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");

    let res = wire.call(9, "session.checkpoints", json!({"id": "s1"}));
    assert_eq!(res["result"]["checkpoints"], json!([]), "{res}");

    let res = wire.call(
        10,
        "session.rename",
        json!({"id": "s1", "title": "Renamed"}),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(11, "session.get", json!({"id": "s1"}));
    assert_eq!(res["result"]["title"], "Renamed", "{res}");

    // Bad params shapes → -32602.
    for (id, method) in [
        (12, "session.get"),
        (13, "session.summary"),
        (14, "session.history"),
        (15, "session.delete"),
        (16, "session.rename"),
        (17, "session.checkpoints"),
    ] {
        let res = wire.call(id, method, json!("not an object"));
        assert_eq!(res["error"]["code"], INVALID_PARAMS, "{method}: {res}");
    }
    let res = wire.call(18, "session.save", json!("not a session"));
    assert_eq!(res["error"]["code"], INVALID_PARAMS, "{res}");

    // memtest has no Rewind / AgentConfig / Restore.
    for (id, method, params) in [
        (19, "session.rewind", json!({})),
        (20, "config.read", json!({})),
        (21, "config.write", json!({"config": {}})),
        (22, "file.restore", json!({})),
    ] {
        let res = wire.call(id, method, params);
        assert_eq!(
            res["error"]["code"], CAPABILITY_UNSUPPORTED,
            "{method}: {res}"
        );
    }
    let res = wire.call(23, "nope", json!({}));
    assert_eq!(res["error"]["code"], METHOD_NOT_FOUND, "{res}");

    let res = wire.call(24, "session.delete", json!({"id": "s1"}));
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(25, "session.get", json!({"id": "s1"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");
}
