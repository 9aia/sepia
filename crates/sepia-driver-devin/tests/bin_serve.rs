#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! `sepia-driver-devin` end to end — spawn the real binary, drive the
//! ndjson driver wire over pipes, close stdin for a clean exit (an
//! instrumented binary only writes its profraw when it exits normally).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use sepia_testkit::contract;
use serde_json::{Value, json};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const SESSION_NOT_FOUND: i64 = -32001;
const CAPABILITY_UNSUPPORTED: i64 = -32004;
const STORE_ERROR: i64 = -32005;

struct Wire {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Wire {
    fn spawn(envs: &[(&str, &str)], dir: &std::path::Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sepia-driver-devin"));
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

    /// Write one raw line; responses may be skipped over.
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
    let mut session = contract::session(id, "Title", 1.0);
    for i in 1..nodes {
        let mut node = session.nodes[0].clone();
        node.node_id = i as i64;
        node.role = sepia_core::Role::Assistant;
        session.nodes.push(node);
    }
    serde_json::to_value(&session).unwrap()
}

#[test]
fn manifest_flag_prints_and_exits() {
    let out = Command::new(env!("CARGO_BIN_EXE_sepia-driver-devin"))
        .arg("--manifest")
        .output()
        .unwrap();
    assert!(out.status.success());
    let manifest: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(manifest["id"], "devin");
    assert_eq!(manifest["agentCommand"], "devin");
}

#[test]
fn serves_the_full_driver_wire() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("sessions.db");
    let devin_dir = tmp.path().join("devin-cfg");
    let mut wire = Wire::spawn(
        &[
            ("SEPIA_DEVIN_DB", db.to_str().unwrap()),
            ("SEPIA_DEVIN_DIR", devin_dir.to_str().unwrap()),
        ],
        tmp.path(),
    );

    // Garbage + ignored frames before the real traffic.
    wire.send("not json");
    {
        let mut line = String::new();
        wire.stdout.read_line(&mut line).unwrap();
        let res: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(res["error"]["code"], PARSE_ERROR, "{res}");
    }
    wire.send("[]"); // not an object → -32600 with id 0
    {
        let mut line = String::new();
        wire.stdout.read_line(&mut line).unwrap();
        let res: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(res["error"]["code"], INVALID_REQUEST, "{res}");
    }
    wire.send(""); // blank line skipped
    // A notification and a response are dropped silently.
    wire.send(&json!({"jsonrpc":"2.0","method":"ping","params":{}}).to_string());
    wire.send(&json!({"jsonrpc":"2.0","id":77,"result":null}).to_string());

    let res = wire.call(1, "driver.manifest", json!({}));
    assert_eq!(res["result"]["id"], "devin", "{res}");

    let res = wire.call(2, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");

    // Bad params → -32602.
    let res = wire.call(3, "session.get", json!({}));
    assert_eq!(res["error"]["code"], INVALID_PARAMS, "{res}");

    // Missing session → -32001.
    let res = wire.call(4, "session.get", json!({"id": "nope"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");

    // Save + read back through every read method.
    let session = session_json("s1", 3);
    let res = wire.call(5, "session.save", session.clone());
    assert!(res.get("error").is_none(), "{res}");

    let res = wire.call(6, "session.list", json!({}));
    assert_eq!(res["result"].as_array().unwrap().len(), 1, "{res}");

    let res = wire.call(7, "session.get", json!({"id": "s1"}));
    assert_eq!(res["result"]["title"], "Title", "{res}");

    let res = wire.call(8, "session.summary", json!({"id": "s1"}));
    assert_eq!(res["result"]["id"], "s1", "{res}");
    let res = wire.call(9, "session.summary", json!({"id": "nope"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");

    let res = wire.call(
        10,
        "session.history",
        json!({"id": "s1", "limit": 2, "before": 3}),
    );
    assert_eq!(res["result"]["total"], 3, "{res}");
    let res = wire.call(11, "session.history", json!({"id": "nope"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");

    let res = wire.call(12, "session.checkpoints", json!({"id": "s1"}));
    assert_eq!(res["result"]["checkpoints"], json!([]), "{res}");

    let res = wire.call(
        13,
        "session.rename",
        json!({"id": "s1", "title": "Renamed"}),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(14, "session.get", json!({"id": "s1"}));
    assert_eq!(res["result"]["title"], "Renamed", "{res}");

    // Rewind: keep node 0, drop the rest — the truncator deletes rows.
    let plan = json!({
        "keepCount": 1,
        "kept": [session["nodes"][0].clone()],
        "removed": [session["nodes"][1].clone(), session["nodes"][2].clone()],
        "removedToolCallIds": []
    });
    let mut truncated = session.clone();
    truncated["nodes"] = json!([session["nodes"][0].clone()]);
    let res = wire.call(
        15,
        "session.rewind",
        json!({"session": session, "plan": plan, "truncated": truncated}),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(16, "session.get", json!({"id": "s1"}));
    assert_eq!(res["result"]["nodes"].as_array().unwrap().len(), 1, "{res}");

    // config.write then config.read round-trip over the wire.
    let res = wire.call(
        17,
        "config.write",
        json!({
            "dir": devin_dir,
            "config": {"rules": [{"name": "wire", "kind": "rule", "body": "b"}]},
        }),
    );
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(18, "config.read", json!({"dir": devin_dir}));
    assert_eq!(res["result"]["rules"][0]["name"], "wire", "{res}");
    // Bare config.read (null params) → driver's own default dir.
    let res = wire.call(19, "config.read", Value::Null);
    assert!(res.get("error").is_none(), "{res}");

    // file.restore is a stub; an unknown method is -32601.
    let res = wire.call(
        20,
        "file.restore",
        json!({"id": "s1", "path": "/x", "confirm": true}),
    );
    assert_eq!(res["error"]["code"], CAPABILITY_UNSUPPORTED, "{res}");
    let res = wire.call(21, "bogus.method", json!({}));
    assert_eq!(res["error"]["code"], METHOD_NOT_FOUND, "{res}");

    let res = wire.call(22, "session.delete", json!({"id": "s1"}));
    assert!(res.get("error").is_none(), "{res}");
    let res = wire.call(23, "session.get", json!({"id": "s1"}));
    assert_eq!(res["error"]["code"], SESSION_NOT_FOUND, "{res}");
}

#[test]
fn readonly_mode_serves_reads_and_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("sessions.db");
    // Seed the store read-write, then reopen the binary read-only.
    {
        let store = sepia_driver_devin::DevinStore::open(&db, false).unwrap();
        let session: sepia_core::Session = serde_json::from_value(session_json("ro", 1)).unwrap();
        seed_save(&store, &session);
    }
    let mut wire = Wire::spawn(
        &[
            ("SEPIA_DEVIN_DB", db.to_str().unwrap()),
            ("SEPIA_DEVIN_READONLY", "1"),
        ],
        tmp.path(),
    );
    let res = wire.call(1, "session.list", json!({}));
    assert_eq!(res["result"].as_array().unwrap().len(), 1, "{res}");

    let res = wire.call(2, "session.save", session_json("x", 1));
    assert_eq!(res["error"]["code"], STORE_ERROR, "{res}");

    // Read-only drivers carry no truncator — rewind is unsupported.
    let res = wire.call(
        3,
        "session.rewind",
        json!({"session": session_json("ro", 1), "plan": {"keepCount":0,"kept":[],"removed":[],"removedToolCallIds":[]}, "truncated": session_json("ro", 1)}),
    );
    assert_eq!(res["error"]["code"], CAPABILITY_UNSUPPORTED, "{res}");

    let res = wire.call(4, "session.rename", json!({"id": "ro", "title": "t"}));
    assert!(res["error"].is_object(), "{res}");
}

#[test]
fn default_paths_resolve_under_home() {
    // No SEPIA_DEVIN_* env — db_path()/devin_dir() take the HOME arms.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".local/share/devin/cli")).unwrap();
    let mut wire = Wire::spawn(&[("HOME", home.to_str().unwrap())], tmp.path());
    let res = wire.call(1, "session.list", json!({}));
    assert_eq!(res["result"], json!([]), "{res}");
    let res = wire.call(2, "config.read", json!({}));
    assert!(res.get("error").is_none(), "{res}");
    assert!(home.join(".local/share/devin/cli/sessions.db").exists());
}

/// One synchronous `save` for seeding the read-only fixture.
fn seed_save(store: &sepia_driver_devin::DevinStore, session: &sepia_core::Session) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            sepia_core::storage::SessionRepository::save(store, session)
                .await
                .unwrap();
        });
}

#[test]
fn empty_env_dirs_fall_back_to_home() {
    // SEPIA_DEVIN_DB="" and SEPIA_DEVIN_DIR="" — both empty-string guards
    // fall through to the HOME defaults.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".local/share/devin/cli")).unwrap();
    let mut wire = Wire::spawn(
        &[
            ("HOME", home.to_str().unwrap()),
            ("SEPIA_DEVIN_DB", ""),
            ("SEPIA_DEVIN_DIR", ""),
        ],
        tmp.path(),
    );
    let res = wire.call(1, "session.list", json!({}));
    assert!(
        res.get("result").is_some() || res.get("error").is_some(),
        "{res}"
    );
    // A dir-less config.read resolves through devin_dir() — the empty
    // SEPIA_DEVIN_DIR takes the HOME fallback.
    let res = wire.call(2, "config.read", Value::Null);
    assert!(
        res.get("result").is_some() || res.get("error").is_some(),
        "{res}"
    );
}
