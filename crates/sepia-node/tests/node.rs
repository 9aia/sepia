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
    let mut session =
        sepia_testkit::contract::session("devin-1", "Node test", 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sepia_core::storage::SessionRepository::save(&store, &session))
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
    let mut body = String::new();
    loop {
        match ureq::get(format!("http://127.0.0.1:{port}/api/sessions")).call() {
            Ok(mut resp) => {
                body = resp.body_mut().read_to_string().unwrap();
                break;
            }
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                panic!("GET /api/sessions never came up: {e}");
            }
        }
    }
    let _ = child.kill();
    assert!(body.contains("devin-1"), "body: {body}");
}
