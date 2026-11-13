#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `sepia serve` — the export/import/convert methods over real ndjson,
//! spawned as `tamias-driver-sepia run serve` (the engine passthrough —
//! identical to the retired `sepia serve` binary), against a
//! shell-script fake `sepia-driver-fake` so `session.save`
//! genuinely crosses the driver wire. Real drivers on the machine stay
//! undiscovered: `SEPIA_DRIVER_DIR` names a dir containing only the
//! fake and `HOME` points at scratch space.

#[cfg(unix)]
mod serve {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

    use sepia_core::storage::SessionRepository;
    use serde_json::{Value, json};

    /// The canned session the fake driver serves for `session.get` /
    /// `session.list` — every required `Session` field present.
    const SESSION: &str = r#"{"id":"s1","title":"Hello","workingDirectory":"/work","model":"m","createdAt":1700000000,"lastActivityAt":1700000001,"mainChainId":0,"nodes":[],"promptHistory":[]}"#;

    /// The canned config IR the fake driver answers `config.read` with.
    const CONFIG: &str = r#"{"rules":[{"name":"house","kind":"rule"}],"mcpServers":{"docs":{"command":"mcp-docs"}},"metadata":{"source":"fake","dir":"/work"}}"#;

    /// Writes a fake driver that prints its manifest on `--manifest`,
    /// then serves ndjson: `session.save` requests are logged to
    /// `<dir>/saves.ndjson`, `config.read`/`config.write` to
    /// `config.ndjson`, `session.get`/`session.list` answer the
    /// canned session, `config.read` the canned config IR — everything
    /// else gets `result:null`.
    fn fake_driver(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let log = dir.join("saves.ndjson");
        let config_log = dir.join("config.ndjson");
        let bin = dir.join("sepia-driver-fake");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--manifest\" ]; then\n\
             \tprintf '%s\\n' '{{\"id\":\"fake\",\"label\":\"Fake\",\"version\":\"0.0.1\",\"protocol\":1,\"capabilities\":[\"sessionStore\",\"sessionWrite\",\"agentConfig\"],\"backendType\":\"fake\"}}'\n\
             \texit 0\n\
             fi\n\
             while IFS= read -r line; do\n\
             \tid=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\),\"method\".*/\\1/p')\n\
             \tcase \"$line\" in\n\
             \t\t*'\"method\":\"session.save\"'*) printf '%s\\n' \"$line\" >> '{log}'; result='null' ;;\n\
             \t\t*'\"method\":\"session.list\"'*) result='[{SESSION}]' ;;\n\
             \t\t*'\"method\":\"session.get\"'*) result='{SESSION}' ;;\n\
             \t\t*'\"method\":\"session.summary\"'*) result='{SESSION}' ;;\n\
             \t\t*'\"method\":\"config.read\"'*) printf '%s\\n' \"$line\" >> '{config_log}'; result='{CONFIG}' ;;\n\
             \t\t*'\"method\":\"config.write\"'*) printf '%s\\n' \"$line\" >> '{config_log}'; result='{{\"actions\":[{{\"path\":\"/out/r.md\",\"action\":\"wrote\"}}]}}' ;;\n\
             \t\t*) result='null' ;;\n\
             \tesac\n\
             \tprintf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":%s}}\\n' \"$id\" \"$result\"\n\
             done\n",
            log = log.display(),
            config_log = config_log.display(),
            SESSION = SESSION,
            CONFIG = CONFIG,
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log, config_log)
    }

    struct Serve {
        child: Child,
        stdin: Option<ChildStdin>,
        stdout: BufReader<ChildStdout>,
    }

    impl Serve {
        /// The engine wire — `run serve` is `sepia serve`.
        fn start(home: &Path, drivers: &Path, envs: &[(&str, &str)]) -> Self {
            Self::launch(home, drivers, envs, &["run", "serve"])
        }

        /// The pavo driver wire — bare `serve` is the ndjson
        /// `{"op":…}` loop (`invoke`/`submit`/`query`/…).
        fn start_driver(home: &Path, drivers: &Path, envs: &[(&str, &str)]) -> Self {
            Self::launch(home, drivers, envs, &["serve"])
        }

        fn launch(home: &Path, drivers: &Path, envs: &[(&str, &str)], argv: &[&str]) -> Self {
            let mut command = Command::new(env!("CARGO_BIN_EXE_tamias-driver-sepia"));
            command
                .args(argv)
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

        /// A raw request line — the pavo driver's wire uses
        /// `{"op":…}` shapes `call`'s method/params framing can't
        /// express. The pavo wire carries no request ids: one
        /// request → exactly one response line.
        fn call_raw(&mut self, _id: u64, request: &str) -> Value {
            let stdin = self.stdin.as_mut().unwrap();
            writeln!(stdin, "{request}").unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "eof");
            serde_json::from_str(&line).unwrap()
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
            // Close stdin first — `serve` exits cleanly on EOF, which
            // lets an instrumented binary flush its coverage profile;
            // a bare kill() would drop the profraw entirely.
            drop(self.stdin.take());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
        let (_, log, _) = fake_driver(&drivers);
        let mut serve = Serve::start(&home, &drivers, &[]);

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
        let mut serve = Serve::start(&home, &drivers, &[]);

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

    /// The params of each `config.*` request the fake driver logged.
    fn logged_config_calls(log: &Path) -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn config_methods_over_the_wire() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        let (_, _, config_log) = fake_driver(&drivers);
        let mut serve = Serve::start(&home, &drivers, &[]);

        // manifest advertises the config surface.
        let res = serve.call(1, "manifest", json!({}));
        for method in ["config.list", "config.get", "config.import"] {
            assert!(
                res["result"]["methods"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(method)),
                "manifest missing {method}: {res}"
            );
        }

        // config.get returns the config IR; `dir` rode the wire.
        let res = serve.call(2, "config.get", json!({"agent": "fake", "dir": "/work"}));
        assert_eq!(res["result"]["rules"][0]["name"], "house", "{res}");
        assert_eq!(
            res["result"]["mcpServers"]["docs"]["command"], "mcp-docs",
            "{res}"
        );
        let calls = logged_config_calls(&config_log);
        assert_eq!(calls[0]["method"], "config.read");
        assert_eq!(calls[0]["params"]["dir"], "/work");

        // config.list inventories every AgentConfig-capable driver.
        let res = serve.call(3, "config.list", json!({}));
        let rows = res["result"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{res}");
        assert_eq!(rows[0]["agent"], "fake");
        assert_eq!(rows[0]["counts"]["rules"], 1);
        assert_eq!(rows[0]["counts"]["mcpServers"], 1);

        // config.list can scope to one agent.
        let res = serve.call(4, "config.list", json!({"agent": "fake"}));
        assert_eq!(res["result"].as_array().unwrap().len(), 1, "{res}");

        // config.import forwards the IR and reports the write actions.
        let res = serve.call(
            5,
            "config.import",
            json!({
                "agent": "fake",
                "dir": "/elsewhere",
                "config": {"rules": [{"name": "installed", "kind": "rule"}]},
            }),
        );
        assert_eq!(res["result"]["actions"][0]["action"], "wrote", "{res}");
        assert_eq!(res["result"]["actions"][0]["path"], "/out/r.md");
        let calls = logged_config_calls(&config_log);
        let write = calls
            .iter()
            .find(|c| c["method"] == "config.write")
            .unwrap();
        assert_eq!(write["params"]["dir"], "/elsewhere");
        assert_eq!(write["params"]["config"]["rules"][0]["name"], "installed");

        serve.call(6, "shutdown", json!({}));
    }

    #[test]
    fn config_error_surface() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut serve = Serve::start(&home, &drivers, &[]);

        // No driver claims the agent → unknown_agent.
        for (id, method, params) in [
            (1, "config.get", json!({"agent": "ghost"})),
            (2, "config.import", json!({"agent": "ghost", "config": {}})),
            (3, "config.list", json!({"agent": "ghost"})),
        ] {
            let res = serve.call(id, method, params);
            assert_eq!(res["error"]["data"]["code"], "unknown_agent", "{res}");
        }

        // Malformed config payload → invalid.
        let res = serve.call(
            4,
            "config.import",
            json!({"agent": "fake", "config": {"rules": "oops"}}),
        );
        assert_eq!(res["error"]["data"]["code"], "invalid", "{res}");

        // `config.get` without an agent → invalid params.
        let res = serve.call(5, "config.get", json!({}));
        assert_eq!(res["error"]["code"], -32602, "{res}");

        serve.call(6, "shutdown", json!({}));
    }

    #[test]
    fn manifest_reports_config_and_events_feed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut serve = Serve::start(
            &home,
            &drivers,
            &[
                ("SEPIA_IDLE_TTL_MS", "4242"),
                ("SEPIA_SWEEP_MS", "777"),
                ("SEPIA_HISTORY_LIMIT", "99"),
                ("SEPIA_LOCK_TTL_MS", "5000"),
            ],
        );

        // The config block reports what the env actually resolved to —
        // a spawner can verify its env took effect. probeIdleMs is the
        // computed fallback: 10 lock TTLs (50s) floored at 60s.
        let res = serve.call(1, "manifest", json!({}));
        let config = &res["result"]["config"];
        assert_eq!(config["idleTtlMs"], 4242, "{res}");
        assert_eq!(config["sweepMs"], 777, "{res}");
        assert_eq!(config["historyLimit"], 99, "{res}");
        assert_eq!(config["probeIdleMs"], 60_000, "{res}");
        assert!(
            res["result"]["methods"]
                .as_array()
                .unwrap()
                .contains(&json!("events.subscribe")),
            "manifest missing events.subscribe: {res}"
        );

        // The node feed subscribes cleanly — nothing live on a plane
        // with no agent runtimes, so the snapshot is empty.
        let res = serve.call(2, "events.subscribe", json!({}));
        assert_eq!(res["result"]["subscribed"], true, "{res}");
        assert_eq!(res["result"]["live"], json!([]), "{res}");

        serve.call(3, "shutdown", json!({}));
    }

    /// The pavo `invoke`/`submit` ops end-to-end: `session.*` action
    /// ids ride the driver's own `sepia serve` child (spawned as
    /// `<self> run serve` — the engine passthrough), `cli.*`-style
    /// ids ride the `run` argv path. Same fake-driver plane as the
    /// engine-wire tests.
    #[test]
    fn invoke_routes_session_and_cli_lanes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut drv = Serve::start_driver(&home, &drivers, &[]);

        // The manifest advertises the declared actions + menus.
        let res = drv.call_raw(1, r#"{"op":"manifest"}"#);
        assert_eq!(res["ok"], true, "{res}");
        let actions = res["result"]["actions"].as_array().unwrap();
        assert!(
            actions.iter().any(|a| a["id"] == "session.prompt"),
            "manifest missing session.prompt: {res}"
        );
        assert!(
            res["result"]["contributes"]["menus"]
                .as_array()
                .is_some_and(|m| !m.is_empty()),
            "manifest missing menus: {res}"
        );

        // Serve lane — `session.export` answers the session IR.
        let res = drv.call_raw(
            2,
            r#"{"op":"invoke","action":"session.export","params":{"id":"s1","agent":"fake"}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"]["data"]["method"], "sessions.export", "{res}");
        assert_eq!(
            res["result"]["data"]["result"]["session"]["id"], "s1",
            "{res}"
        );

        // Serve lane — `session.patch_meta` writes the overlay.
        let res = drv.call_raw(
            3,
            r#"{"op":"invoke","action":"session.patch_meta","params":{"id":"s1","title":"Renamed"}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(
            res["result"]["data"]["method"], "sessions.patch_meta",
            "{res}"
        );

        // Required-param gating — no id, no wire call.
        let res = drv.call_raw(
            4,
            r#"{"op":"invoke","action":"session.delete","params":{}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"]["errors"]["id"], "required", "{res}");

        // Scalar target fills `id` — a tree-item/row selection shape.
        let res = drv.call_raw(
            5,
            r#"{"op":"invoke","action":"session.export","target":"s1"}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(
            res["result"]["data"]["result"]["session"]["id"], "s1",
            "{res}"
        );

        // Cli lane — `version` is an engine verb, not a serve method.
        let res = drv.call_raw(6, r#"{"op":"invoke","action":"version"}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"]["data"]["op"], "version", "{res}");
        assert_eq!(res["result"]["data"]["code"], 0, "{res}");

        // Unknown action → wire ok:false.
        let res = drv.call_raw(7, r#"{"op":"invoke","action":"session.bogus"}"#);
        assert_eq!(res["ok"], false, "{res}");

        // Submit adapter — a view/form naming an action dispatches.
        let res = drv.call_raw(
            8,
            r#"{"op":"submit","view":"session.export","values":{"id":"s1","agent":"fake"}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(
            res["result"]["data"]["result"]["session"]["id"], "s1",
            "{res}"
        );
        let res = drv.call_raw(9, r#"{"op":"submit","view":"bogus","values":{}}"#);
        assert_eq!(res["ok"], false, "{res}");
    }

    /// The P2 views over the wire: `config.*` queries hit the fake
    /// driver's `config.read`, `stores`/`drivers` degrade to honest
    /// rows/output under the test env, and the session meta form
    /// submits through `sessions.patch_meta` with the view's
    /// resolved id/agent.
    #[test]
    fn p2_views_and_meta_form_submit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut drv = Serve::start_driver(&home, &drivers, &[]);

        // `config.agents` — config.list row merged into the table shape.
        let res = drv.call_raw(1, r#"{"op":"query","query":"config.agents","params":{}}"#);
        assert_eq!(res["ok"], true, "{res}");
        let rows = res["result"]["data"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{res}");
        assert_eq!(rows[0]["id"], "fake");
        assert_eq!(rows[0]["from"], "fake");
        assert_eq!(rows[0]["rules"], 1);
        assert_eq!(rows[0]["mcpServers"], 1);

        // `config.detail` — config.get flattened into tree items.
        let res = drv.call_raw(
            2,
            r#"{"op":"query","query":"config.detail","params":{"agent":"fake"}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        let items = res["result"]["data"]["items"].as_array().unwrap();
        let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert!(labels.contains(&"Rules (1)"), "{labels:?}");
        assert!(labels.contains(&"MCP servers (1)"), "{labels:?}");
        let rules = items.iter().find(|i| i["label"] == "Rules (1)").unwrap();
        assert_eq!(rules["children"][0]["label"], "house");

        // The `config` view binds both queries; the detail defaults to
        // the first config-capable agent when `agent` is absent.
        let res = drv.call_raw(3, r#"{"op":"view","view":"config","params":{}}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert!(
            res["result"]["data"]["config.detail"]["items"]
                .as_array()
                .is_some_and(|i| !i.is_empty()),
            "{res}"
        );

        // `stores` — `store list` per store through the cli lane.
        // Seed a devin db and pass `--param db=` (forwarded to every
        // store's flags; only devin reads it); the other stores hit
        // missing default dirs under the test HOME → error rows or
        // empty. The devin row must parse out of the TSV.
        let db = tmp.path().join("sessions.db");
        {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let store = sepia_driver_devin::DevinStore::open(&db, false).unwrap();
                store
                    .save(&sepia_testkit::contract::session(
                        "store-s1",
                        "store session",
                        2.0,
                    ))
                    .await
                    .unwrap();
            });
        }
        let res = drv.call_raw(
            4,
            &format!(
                r#"{{"op":"query","query":"stores","params":{{"db":"{}"}}}}"#,
                db.display()
            ),
        );
        assert_eq!(res["ok"], true, "{res}");
        let rows = res["result"]["data"].as_array().unwrap();
        assert!(!rows.is_empty(), "{res}");
        let devin = rows.iter().find(|r| r["store"] == "devin");
        assert!(devin.is_some(), "no devin row in {rows:?}");
        let devin = devin.unwrap();
        assert_eq!(devin["id"], "store-s1", "{devin}");
        assert_eq!(devin["session_id"], "store-s1");
        assert_eq!(devin["from"], "devin");
        assert_eq!(devin["title"], "store session");
        for row in rows {
            assert!(
                ["devin", "cline", "claude", "cursor"].contains(&row["store"].as_str().unwrap()),
                "{row}"
            );
        }

        // `drivers` — `driver list --json` through the cli lane; the
        // run children inherit SEPIA_DRIVER_DIR, so the fake driver
        // shows up as a row.
        let res = drv.call_raw(5, r#"{"op":"query","query":"drivers","params":{}}"#);
        assert_eq!(res["ok"], true, "{res}");
        let rows = res["result"]["data"].as_array().unwrap();
        assert!(
            rows.iter().any(|r| r["id"] == "fake"),
            "fake driver missing from {rows:?}"
        );

        // The meta form: resolve the session view (sets the submit
        // context), then submit form "meta" — values carry only the
        // editable fields; id/agent come from the view resolution.
        let res = drv.call_raw(
            6,
            r#"{"op":"view","view":"session","params":{"id":"s1","agent":"fake"}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        // the returned spec's form fields are seeded from the detail
        assert!(
            res["result"]["data"]["session"]["title"].is_string(),
            "session detail missing title: {}",
            res["result"]["data"]["session"]
        );
        let form = res["result"]["view"]["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["kind"] == "form")
            .unwrap();
        assert_eq!(form["id"], "meta");
        assert_eq!(form["submit_op"], "session.patch_meta");
        assert_eq!(form["fields"][0]["key"], "title");
        assert_eq!(form["fields"][0]["value"], "Hello");

        let res = drv.call_raw(
            7,
            r#"{"op":"submit","view":"session","form":"meta","values":{"title":"Renamed via form","pinned":true}}"#,
        );
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(
            res["result"]["data"]["method"], "sessions.patch_meta",
            "{res}"
        );
    }

    /// P3 — the live event feed: `serve` warms a second `sepia
    /// serve` child on `events.subscribe` (self-resolution →
    /// `run serve`, same scratch env). `feed.status`/`live`/
    /// `permissions` queries and the `jobs`/`notifications` ops
    /// answer honestly on an empty plane.
    #[test]
    fn live_feed_queries_and_platform_ops_over_the_wire() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let drivers = tmp.path().join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        fake_driver(&drivers);
        let mut drv = Serve::start_driver(&home, &drivers, &[]);

        // The feed child's plane build overlaps the handshake —
        // poll `feed.status` until the subscribe ack lands.
        let mut status = Value::Null;
        for _ in 0..100 {
            let res = drv.call_raw(1, r#"{"op":"query","query":"feed.status","params":{}}"#);
            assert_eq!(res["ok"], true, "{res}");
            status = res["result"]["data"].clone();
            if status["status"].as_str() == Some("live") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert_eq!(status["status"], "live", "{status}");
        assert!(status["sessions"].is_u64(), "{status}");

        // `live`/`permissions` — empty feed rows, still `ok:true`.
        let res = drv.call_raw(2, r#"{"op":"query","query":"live","params":{}}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"]["data"], json!([]), "{res}");
        let res = drv.call_raw(3, r#"{"op":"query","query":"permissions","params":{}}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"]["data"], json!([]), "{res}");

        // Platform feeds — no busy sessions, no pending asks, feed
        // healthy → both ops return empty vectors.
        let res = drv.call_raw(4, r#"{"op":"jobs"}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"], json!([]), "{res}");
        let res = drv.call_raw(5, r#"{"op":"notifications"}"#);
        assert_eq!(res["ok"], true, "{res}");
        assert_eq!(res["result"], json!([]), "{res}");
    }
}
