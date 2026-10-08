#![allow(clippy::unwrap_used, clippy::pedantic)]

//! End-to-end: the sepia-node binary discovers a real driver on
//! SEPIA_DRIVER_DIR, merges its store, and serves /api/sessions.

#[test]
fn node_serves_sessions_over_http() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let home = tmp.path().join("home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();

    let db = db_dir.join("sessions.db");
    let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
    let mut session = sepia_testkit::contract::session("devin-1", "Node test", 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sepia_core::storage::SessionRepository::save(
            &store, &session,
        ))
        .unwrap();

    let port = 18787u16;
    let mut child = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &home)
        .env("SEPIA_META", home.join("meta.json"))
        .env("SEPIA_NODE", home.join("node.json"))
        .env("SEPIA_PORT", port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let body = loop {
        match ureq::get(format!("http://127.0.0.1:{port}/api/sessions")).call() {
            Ok(mut resp) => break resp.body_mut().read_to_string().unwrap(),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
    let _ = child.wait();
                let _ = child.wait();
                panic!("GET /api/sessions never came up: {e}");
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(body.contains("devin-1"), "body: {body}");

    // RSS smoke budget — a headless node must idle small (plan's
    // lightweight rule; adjust if real work lands, never silently).
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{}/status", child.id())) {
        let rss_kb = status
            .lines()
            .find(|l| l.starts_with("VmRSS"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        assert!(
            rss_kb < 100 * 1024,
            "node RSS {rss_kb} kB over 100 MB budget"
        );
    }
}

/// Full stack: node -> driver store + mock ACP agent over HTTP.
/// attach -> prompt -> SSE stream sees the echo chunk.
#[test]
fn node_attach_prompt_stream_e2e() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let home = tmp.path().join("home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();
    let mock = sepia_testkit::ensure_mock_acp_agent();

    let db = db_dir.join("sessions.db");
    let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
    let mut session = sepia_testkit::contract::session("devin-1", "Live test", 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    session.working_directory = "/tmp".into();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sepia_core::storage::SessionRepository::save(
            &store, &session,
        ))
        .unwrap();

    let port = 18788u16;
    let mut child = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &home)
        .env("SEPIA_META", home.join("meta.json"))
        .env("SEPIA_NODE", home.join("node.json"))
        .env("SEPIA_PORT", port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("SEPIA_AGENT_DEVIN_COMMAND", mock.to_str().unwrap())
        .env("MOCK_CAPS", "load,list")
        .env(
            "MOCK_SESSIONS",
            "devin-1|/tmp|Live test|2024-01-01T00:00:00Z",
        )
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let client = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let base = format!("http://127.0.0.1:{port}");
    // Wait for health.
    loop {
        match client.get(format!("{base}/api/health")).call() {
            Ok(_) => break,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
    let _ = child.wait();
                let _ = child.wait();
                panic!("node never came up: {e}");
            }
        }
    }

    // Attach -> the plane loads the session on the mock agent.
    let resp = client
        .post(format!("{base}/api/sessions/devin-1/attach"))
        .send_json(serde_json::json!({}));
    assert!(resp.is_ok(), "attach failed: {resp:?}");

    // Subscribe BEFORE prompting — the broadcast drops pre-subscribe
    // events.
    let mut resp = client
        .get(format!("{base}/api/sessions/devin-1/stream"))
        .call()
        .unwrap();

    // Prompt -> mock echoes -> RunFinished.
    let resp2 = client
        .post(format!("{base}/api/sessions/devin-1/prompt"))
        .send_json(serde_json::json!({ "text": "hello" }));
    assert!(resp2.is_ok(), "prompt failed: {resp2:?}");

    let mut body = String::new();
    let mut buf = [0u8; 8192];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !body.contains("echo: hello") && std::time::Instant::now() < deadline {
        match std::io::Read::read(&mut resp.body_mut().as_reader(), &mut buf) {
            Ok(0) => break,
            Ok(n) => body.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(body.contains("echo: hello"), "stream body: {body}");
}
