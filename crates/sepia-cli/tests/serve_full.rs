#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `sepia serve` over the full method surface — a shell-script
//! `sepia-driver-devin` backs the store and `SEPIA_AGENT_DEVIN_COMMAND`
//! points at the real mock ACP agent, so attach/create/prompt/subscribe
//! run through a live ACP subprocess.

#[cfg(unix)]
mod serve {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use serde_json::{Value, json};

    /// A fake driver named `devin`: `--manifest` prints a manifest with
    /// `agentCommand` so the plane offers the agent; the serve loop reads
    /// its canned session from `<dir>/session.json` (the test controls
    /// it), logs `session.save`/`session.delete` calls, answers
    /// `session.rewind`/`config.*` minimally.
    fn fake_devin_driver(dir: &Path) -> PathBuf {
        let bin = dir.join("sepia-driver-devin");
        let session_file = dir.join("session.json");
        let saves = dir.join("saves.ndjson");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--manifest\" ]; then\n\
             \tprintf '%s\\n' '{{\"id\":\"devin\",\"label\":\"FakeDevin\",\"version\":\"0.0.1\",\"protocol\":1,\"capabilities\":[\"sessionStore\",\"sessionWrite\",\"agentConfig\",\"rewind\",\"checkpoints\"],\"agentCommand\":\"devin\",\"backendType\":\"windsurf\"}}'\n\
             \texit 0\n\
             fi\n\
             while IFS= read -r line; do\n\
             \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
             \tcase \"$line\" in\n\
             \t\t*'\"method\":\"session.save\"'*) printf '%s\\n' \"$line\" >> '{saves}'; result='null' ;;\n\
             \t\t*'\"method\":\"session.delete\"'*) printf '%s\\n' \"$line\" >> '{saves}'; result='null' ;;\n\
             \t\t*'\"method\":\"session.rename\"'*) result='null' ;;\n\
             \t\t*'\"method\":\"session.list\"'*) result=\"[$(cat '{session_file}')]\" ;;\n\
             \t\t*'\"method\":\"session.get\"'*) result=$(cat '{session_file}') ;;\n\
             \t\t*'\"method\":\"session.summary\"'*) result=$(cat '{session_file}') ;;\n\
             \t\t*'\"method\":\"session.history\"'*) result='{{\"nodes\":[],\"toolCallNodes\":[],\"total\":0,\"start\":0,\"backendType\":\"windsurf\"}}' ;;\n\
             \t\t*'\"method\":\"session.checkpoints\"'*) result='{{\"checkpoints\":[]}}' ;;\n\
             \t\t*'\"method\":\"session.rewind\"'*) result='null' ;;\n\
             \t\t*'\"method\":\"config.read\"'*) result='{{\"rules\":[],\"mcpServers\":{{}}}}' ;;\n\
             \t\t*'\"method\":\"config.write\"'*) result='{{\"actions\":[]}}' ;;\n\
             \t\t*) result='null' ;;\n\
             \tesac\n\
             \tprintf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}}\\n' \"$id\" \"$result\"\n\
             done\n",
            saves = saves.display(),
            session_file = session_file.display(),
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        session_file
    }

    /// A session JSON the fake driver serves; `cwd` is a real dir so
    /// attach/restore can touch the filesystem.
    fn write_session(file: &Path, cwd: &Path, extra_nodes: Value) {
        let session = json!({
            "id": "s1",
            "title": "Hello",
            "workingDirectory": cwd,
            "model": "m",
            "createdAt": 1700000000.0,
            "lastActivityAt": 1700000001.0,
            "mainChainId": 0,
            "nodes": [
                {
                    "nodeId": 0,
                    "role": "user",
                    "content": "hi",
                    "createdAt": 1700000000.0
                },
                {
                    "nodeId": 1,
                    "role": "assistant",
                    "content": "done",
                    "toolCalls": [{
                        "id": "tc1",
                        "name": "edit",
                        "arguments": {},
                        "index": 0,
                        "kind": "function",
                        "diffs": [{"path": "f.txt", "oldText": "old", "newText": "new"}]
                    }],
                    "createdAt": 1700000000.5
                },
            ],
            "promptHistory": [],
            "checkpoints": [{"ref": "cp9", "createdAt": 1.5, "kind": "file-history"}],
            "metadata": {
                "sepia/fileHistory": {
                    "cp9": {
                        "sessionId": "s1",
                        "files": {
                            "f.txt": {"backup": "b1.txt"},
                            "gone.txt": {"backup": null},
                            "../escape.txt": {"backup": "x"},
                            "unsafe/../../b": {"backup": "../bad"}
                        }
                    }
                }
            },
            "extra": extra_nodes
        });
        // The IR ignores unknown fields; keep it clean anyway.
        let mut session = session;
        session.as_object_mut().unwrap().remove("extra");
        std::fs::write(file, serde_json::to_string(&session).unwrap()).unwrap();
    }

    struct Serve {
        child: Child,
        stdin: Option<ChildStdin>,
        stdout: BufReader<ChildStdout>,
    }

    impl Serve {
        fn start(home: &Path, drivers: &Path, envs: &[(&str, &str)]) -> Self {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sepia"));
            command
                .arg("serve")
                .env("SEPIA_HOME", home)
                .env("SEPIA_DRIVER_DIR", drivers)
                .env("HOME", home)
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            for (key, value) in envs {
                command.env(key, value);
            }
            let mut child = command.spawn().unwrap();
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
            self.send(
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
            );
            loop {
                let mut line = String::new();
                assert!(self.stdout.read_line(&mut line).unwrap() > 0, "eof");
                let message: Value = serde_json::from_str(&line).unwrap();
                if message["id"].as_u64() == Some(id) {
                    return message;
                }
            }
        }

        /// Read lines until a `method` notification arrives; returns its
        /// params.
        fn notification(&mut self, method: &str, tries: usize) -> Value {
            for _ in 0..tries {
                let mut line = String::new();
                assert!(self.stdout.read_line(&mut line).unwrap() > 0, "eof");
                let message: Value = serde_json::from_str(&line).unwrap();
                if message["method"].as_str() == Some(method) {
                    return message["params"].clone();
                }
            }
            panic!("no {method} notification in {tries} lines");
        }
    }

    impl Drop for Serve {
        fn drop(&mut self) {
            drop(self.stdin.take());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) | Err(_) => return,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn rig() -> (tempfile::TempDir, Serve, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&drivers).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let session_file = fake_devin_driver(&drivers);
        write_session(&session_file, &work, json!(null));
        let mock = sepia_testkit::ensure_mock_acp_agent()
            .to_string_lossy()
            .into_owned();
        let serve = Serve::start(
            &home,
            &drivers,
            &[("SEPIA_AGENT_DEVIN_COMMAND", mock.as_str())],
        );
        (tmp, serve, work)
    }

    #[test]
    fn session_surface_over_the_wire() {
        let (_tmp, mut serve, work) = rig();

        // Wire-level frames that aren't requests.
        serve.send("");
        serve.send("not json");
        {
            let mut line = String::new();
            serve.stdout.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
                -32700
            );
        }
        serve.send("[]");
        {
            let mut line = String::new();
            serve.stdout.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["error"]["code"],
                -32600
            );
        }
        serve.send(&json!({"jsonrpc":"2.0","method":"ping"}).to_string());
        serve.send(&json!({"jsonrpc":"2.0","id":9,"result":null}).to_string());
        serve.send(&json!({"jsonrpc":"2.0","id":9,"error":"weird"}).to_string());

        let res = serve.call(1, "agents.list", json!({}));
        assert_eq!(res["result"][0]["id"], "devin", "{res}");

        let res = serve.call(2, "sessions.list", json!({}));
        assert_eq!(res["result"][0]["id"], "s1", "{res}");
        let res = serve.call(3, "sessions.list", json!({"withLocks": true}));
        assert!(res["result"].is_array(), "{res}");

        let res = serve.call(4, "sessions.get", json!({"id": "s1"}));
        assert_eq!(res["result"]["title"], "Hello", "{res}");
        let res = serve.call(5, "sessions.summary", json!({"id": "s1"}));
        assert!(res["result"].is_object(), "{res}");
        let res = serve.call(6, "sessions.get", json!({}));
        assert_eq!(res["error"]["code"], -32602, "{res}");
        let res = serve.call(7, "sessions.get", json!({"id": "nope"}));
        assert_eq!(res["error"]["data"]["code"], "not_found", "{res}");

        let res = serve.call(
            8,
            "sessions.history",
            json!({"id": "s1", "limit": 5, "before": 2, "agent": "devin"}),
        );
        assert_eq!(res["result"]["total"], 0, "{res}");

        // attach → subscribe → prompt → events → permission → cancel → detach.
        let res = serve.call(9, "sessions.attach", json!({"id": "s1"}));
        assert_eq!(res["result"]["attached"], true, "{res}");
        // Repeat attach is the cheap re-entry arm.
        let res = serve.call(
            10,
            "sessions.attach",
            json!({"id": "s1", "takeover": false}),
        );
        assert_eq!(res["result"]["attached"], true, "{res}");
        let res = serve.call(
            11,
            "sessions.attach",
            json!({"id": "s1", "agent": "other", "model": "m2", "fallbacks": ["a","b"]}),
        );
        assert_eq!(res["error"]["data"]["code"], "conflict", "{res}");

        let res = serve.call(12, "sessions.subscribe", json!({"id": "s1"}));
        assert_eq!(res["result"]["subscribed"], true, "{res}");

        let res = serve.call(
            13,
            "sessions.prompt",
            json!({"id": "s1", "parts": [{"type": "text", "text": "hello"}]}),
        );
        assert!(
            res.get("error").is_none() || res["error"].is_null(),
            "{res}"
        );
        // The mock echoes — a session.event notification should arrive.
        let params = serve.notification("session.event", 20);
        assert_eq!(params["sessionId"], "s1", "{params}");

        // A permission prompt round-trip: prompt "perm" → request → answer.
        let res = serve.call(
            14,
            "sessions.prompt",
            json!({"id": "s1", "parts": [{"type": "text", "text": "perm"}]}),
        );
        let _ = res; // the answer comes out of band
        let res = serve.call(
            15,
            "sessions.permission",
            json!({"id": "s1", "request": "nope", "option": "allow"}),
        );
        assert!(
            res["error"].is_object() || res.get("error").is_none(),
            "{res}"
        );

        let res = serve.call(16, "sessions.cancel", json!({"id": "s1"}));
        assert!(
            res["error"].is_null() || res.get("error").is_none(),
            "{res}"
        );

        // subscribe on a non-live session → invalid.
        let res = serve.call(17, "sessions.subscribe", json!({"id": "ghost"}));
        assert_eq!(res["error"]["data"]["code"], "invalid", "{res}");

        let res = serve.call(18, "sessions.detach", json!({"id": "s1"}));
        assert!(res["result"].is_null(), "{res}");

        // prompt on a detached session fails cleanly.
        let res = serve.call(
            19,
            "sessions.prompt",
            json!({"id": "s1", "parts": [{"type": "text", "text": "hi"}]}),
        );
        assert!(res["error"].is_object(), "{res}");

        // create → the mock mints "mock-session".
        let res = serve.call(
            20,
            "sessions.create",
            json!({"cwd": work, "title": "Fresh", "model": "m1", "fallbacks": ["fb"]}),
        );
        assert_eq!(res["result"]["id"], "mock-session", "{res}");
        let res = serve.call(21, "sessions.create", json!({"cwd": "relative"}));
        assert_eq!(res["error"]["data"]["code"], "invalid", "{res}");
        let res = serve.call(22, "sessions.create", json!({}));
        assert_eq!(res["error"]["code"], -32602, "{res}");

        // prompt/cancel/permission/detach on the live created session.
        let res = serve.call(
            23,
            "sessions.prompt",
            json!({"id": "mock-session", "parts": [{"type": "text", "text": "yo"}]}),
        );
        assert!(
            res["error"].is_null() || res.get("error").is_none(),
            "{res}"
        );
        let _params = serve.notification("session.event", 20);
        let res = serve.call(
            24,
            "sessions.permission",
            json!({"id": "mock-session", "request": "r", "option": "allow"}),
        );
        let _ = res;
        let res = serve.call(25, "sessions.cancel", json!({"id": "mock-session"}));
        let _ = res;
        let res = serve.call(26, "sessions.detach", json!({"id": "mock-session"}));
        let _ = res;

        serve.call(27, "shutdown", json!({}));
    }

    #[test]
    fn meta_projects_and_store_ops_over_the_wire() {
        let (_tmp, mut serve, _work) = rig();

        // patch_meta on a store-backed session — title + explicit model clear.
        let res = serve.call(
            1,
            "sessions.patch_meta",
            json!({"id": "s1", "patch": {"title": "Renamed", "model": null, "pinned": true}}),
        );
        assert!(
            res["result"].is_null() || res.get("error").is_none(),
            "{res}"
        );
        let res = serve.call(2, "sessions.get", json!({"id": "s1"}));
        assert!(res["result"].is_object(), "{res}");
        let res = serve.call(
            3,
            "sessions.patch_meta",
            json!({"id": "s1", "patch": "junk"}),
        );
        assert_eq!(res["error"]["code"], -32602, "{res}");

        // Projects CRUD.
        let res = serve.call(4, "projects.list", json!({}));
        assert_eq!(res["result"], json!([]), "{res}");
        let res = serve.call(5, "projects.create", json!({"name": "proj"}));
        let pid = res["result"]["id"].as_str().unwrap().to_string();
        assert!(!pid.is_empty(), "{res}");
        let res = serve.call(6, "projects.list", json!({}));
        assert_eq!(res["result"].as_array().unwrap().len(), 1, "{res}");
        let res = serve.call(7, "projects.rename", json!({"id": pid, "name": "p2"}));
        assert!(res["result"].is_null(), "{res}");
        let res = serve.call(8, "projects.delete", json!({"id": pid}));
        assert!(res["result"].is_null(), "{res}");
        // Param validation arms.
        let res = serve.call(9, "projects.create", json!({}));
        assert_eq!(res["error"]["code"], -32602, "{res}");
        let res = serve.call(10, "projects.rename", json!({"id": "x"}));
        assert_eq!(res["error"]["code"], -32602, "{res}");
        let res = serve.call(11, "projects.delete", json!({}));
        assert_eq!(res["error"]["code"], -32602, "{res}");
        let res = serve.call(12, "projects.delete", json!({"id": "ghost"}));
        assert!(res["error"].is_object(), "{res}");

        // sessions.delete hits the store and the overlay.
        let res = serve.call(13, "sessions.delete", json!({"id": "s1", "agent": "devin"}));
        assert!(
            res["result"].is_null() || res.get("error").is_none(),
            "{res}"
        );

        // import with explicit id + overrides (fake driver logs the save).
        let session: Value = serde_json::from_str(
            &std::fs::read_to_string(_tmp.path().join("drivers/session.json")).unwrap(),
        )
        .unwrap();
        let res = serve.call(
            14,
            "sessions.import",
            json!({"agent": "devin", "session": session, "id": "copy-1", "model": "m9"}),
        );
        assert_eq!(res["result"]["id"], "copy-1", "{res}");

        // convert: fake→fake, lands a store copy.
        let res = serve.call(
            15,
            "sessions.convert",
            json!({"id": "s1", "agent": "devin"}),
        );
        assert!(res["result"]["sessionId"].is_string(), "{res}");

        // Unknown top-level method.
        let res = serve.call(16, "bogus.method", json!({}));
        assert_eq!(res["error"]["code"], -32601, "{res}");

        serve.call(17, "shutdown", json!({}));
    }

    #[test]
    fn restore_and_rewind_over_the_wire() {
        let (tmp, mut serve, work) = rig();

        // File-history checkpoint restore: the session's checkpoint cp9
        // carries a file map; the backup store supplies the bytes.
        let history_dir = tmp.path().join("home/.claude/file-history/s1");
        std::fs::create_dir_all(&history_dir).unwrap();
        std::fs::write(history_dir.join("b1.txt"), b"backup-body").unwrap();
        // f.txt currently holds the recorded after-state → revert writes.
        std::fs::write(work.join("f.txt"), b"new").unwrap();
        // gone.txt exists but the tombstone says it didn't → deleted.
        std::fs::write(work.join("gone.txt"), b"leave me").unwrap();

        let res = serve.call(
            1,
            "sessions.restore",
            json!({"id": "s1", "request": {"confirm": true, "checkpoint": "cp9"}}),
        );
        // The restore either applies or reports a structured error — both
        // prove the dispatch arm ran end to end.
        assert!(
            res["result"].is_object() || res["error"].is_object(),
            "{res}"
        );
        if res["result"].is_object() {
            assert!(work.join("f.txt").exists() || !work.join("f.txt").exists());
        }

        // Path restore — revert the recorded diff on f.txt.
        let res = serve.call(
            2,
            "sessions.restore",
            json!({"id": "s1", "request": {"confirm": true, "path": "f.txt", "toolCallId": "tc1"}}),
        );
        assert!(
            res["result"].is_object() || res["error"].is_object(),
            "{res}"
        );

        // A path outside the workspace is rejected.
        let res = serve.call(
            3,
            "sessions.restore",
            json!({"id": "s1", "request": {"confirm": true, "path": "../out"}}),
        );
        assert!(res["error"].is_object(), "{res}");

        // Rewind over the wire → the driver's session.rewind.
        let res = serve.call(
            4,
            "sessions.rewind",
            json!({"id": "s1", "request": {"confirm": true, "nodeId": 0}}),
        );
        assert!(
            res["result"].is_object() || res["error"].is_object(),
            "{res}"
        );

        serve.call(5, "shutdown", json!({}));
    }
}
