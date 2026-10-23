#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `sepia serve` — the export/import/convert methods over real ndjson,
//! against a shell-script fake `sepia-driver-fake` so `session.save`
//! genuinely crosses the driver wire. Real drivers on the machine stay
//! undiscovered: `SEPIA_DRIVER_DIR` names a dir containing only the
//! fake and `HOME` points at scratch space.

#[cfg(unix)]
mod serve {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use serde_json::{Value, json};

    /// The canned session the fake driver serves for `session.get` /
    /// `session.list` — every required `Session` field present.
    const SESSION: &str = r#"{"id":"s1","title":"Hello","workingDirectory":"/work","model":"m","createdAt":1700000000,"lastActivityAt":1700000001,"mainChainId":0,"nodes":[],"promptHistory":[]}"#;

    /// Writes a fake driver that prints its manifest on `--manifest`,
    /// then serves ndjson: `session.save` requests are logged to
    /// `<dir>/saves.ndjson`, `session.get`/`session.list` answer the
    /// canned session, everything else gets `result:null`.
    fn fake_driver(dir: &Path) -> (PathBuf, PathBuf) {
        let log = dir.join("saves.ndjson");
        let bin = dir.join("sepia-driver-fake");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--manifest\" ]; then\n\
             \tprintf '%s\\n' '{{\"id\":\"fake\",\"label\":\"Fake\",\"version\":\"0.0.1\",\"protocol\":1,\"capabilities\":[\"sessionStore\",\"sessionWrite\"],\"backendType\":\"fake\"}}'\n\
             \texit 0\n\
             fi\n\
             while IFS= read -r line; do\n\
             \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
             \tcase \"$line\" in\n\
             \t\t*'\"method\":\"session.save\"'*) printf '%s\\n' \"$line\" >> '{log}'; result='null' ;;\n\
             \t\t*'\"method\":\"session.list\"'*) result='[{SESSION}]' ;;\n\
             \t\t*'\"method\":\"session.get\"'*) result='{SESSION}' ;;\n\
             \t\t*) result='null' ;;\n\
             \tesac\n\
             \tprintf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}}\\n' \"$id\" \"$result\"\n\
             done\n",
            log = log.display(),
            SESSION = SESSION,
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    struct Serve {
        child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    }

    impl Serve {
        fn start(home: &Path, drivers: &Path) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_sepia"))
                .arg("serve")
                .env("SEPIA_HOME", home)
                .env("SEPIA_DRIVER_DIR", drivers)
                .env("HOME", home)
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                stdin: child.stdin.take().unwrap(),
                stdout: BufReader::new(child.stdout.take().unwrap()),
                child,
            }
        }

        fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
            writeln!(
                self.stdin,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .unwrap();
            self.stdin.flush().unwrap();
            loop {
                let mut line = String::new();
                assert!(self.stdout.read_line(&mut line).unwrap() > 0, "eof");
                let message: Value = serde_json::from_str(&line).unwrap();
                // Skip notifications (session.event) — wait for our id.
                if message["id"].as_u64() == Some(id) {
                    return message;
                }
            }
        }
    }

    impl Drop for Serve {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn saved_sessions(log: &Path) -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["params"].clone())
            .collect()
    }

    #[test]
    fn export_import_convert_over_the_wire() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        let (_, log) = fake_driver(&drivers);
        let mut serve = Serve::start(&home, &drivers);

        // manifest advertises the transfer surface.
        let res = serve.call(1, "manifest", json!({}));
        for method in ["sessions.export", "sessions.import", "sessions.convert"] {
            assert!(
                res["result"]["methods"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(method)),
                "manifest missing {method}: {res}"
            );
        }

        // export: the transfer-bundle shape.
        let res = serve.call(2, "sessions.export", json!({"id": "s1", "agent": "fake"}));
        assert_eq!(res["result"]["session"]["id"], "s1", "{res}");
        assert_eq!(res["result"]["session"]["title"], "Hello", "{res}");

        // import: mints a fresh id, applies overrides, saves via the wire.
        let res = serve.call(
            3,
            "sessions.import",
            json!({
                "agent": "fake",
                "session": serde_json::from_str::<Value>(SESSION).unwrap(),
                "cwd": "/elsewhere",
                "title": "Imported copy",
            }),
        );
        let imported_id = res["result"]["id"].as_str().unwrap().to_string();
        assert_ne!(imported_id, "s1");
        assert_eq!(res["result"]["agentId"], "fake", "{res}");
        let saved = saved_sessions(&log);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0]["id"], imported_id);
        assert_eq!(saved[0]["workingDirectory"], "/elsewhere");
        assert_eq!(saved[0]["title"], "Imported copy");

        // convert: reads the merged store, lands a second copy.
        let res = serve.call(
            4,
            "sessions.convert",
            json!({"id": "s1", "agent": "fake", "from": "fake"}),
        );
        let converted_id = res["result"]["sessionId"].as_str().unwrap().to_string();
        assert_ne!(converted_id, "s1");
        assert_eq!(res["result"]["agentId"], "fake", "{res}");
        let saved = saved_sessions(&log);
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1]["id"], converted_id);
        assert_eq!(saved[1]["title"], "Hello");

        let res = serve.call(5, "shutdown", json!({}));
        assert!(
            res["error"].is_null() || res.get("error").is_none(),
            "{res}"
        );
    }

    #[test]
    fn import_error_surface() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut serve = Serve::start(&home, &drivers);

        // No SessionWrite driver for the agent → unknown_agent.
        let res = serve.call(
            1,
            "sessions.import",
            json!({
                "agent": "ghost",
                "session": serde_json::from_str::<Value>(SESSION).unwrap(),
            }),
        );
        assert_eq!(res["error"]["data"]["code"], "unknown_agent", "{res}");

        // Malformed session payload → invalid.
        let res = serve.call(
            2,
            "sessions.import",
            json!({"agent": "fake", "session": {"not": "a session"}}),
        );
        assert_eq!(res["error"]["data"]["code"], "invalid", "{res}");

        // A caller-chosen id rides through.
        let res = serve.call(
            3,
            "sessions.import",
            json!({
                "agent": "fake",
                "session": serde_json::from_str::<Value>(SESSION).unwrap(),
                "id": "chosen-9",
            }),
        );
        assert_eq!(res["result"]["id"], "chosen-9", "{res}");

        serve.call(4, "shutdown", json!({}));
    }
}
