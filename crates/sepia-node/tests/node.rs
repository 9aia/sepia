#![allow(clippy::unwrap_used, clippy::pedantic)]

//! End-to-end: the sepia-node binary discovers a real driver on
//! SEPIA_DRIVER_DIR, merges its store, and reports the session count.

#[test]
fn node_discovers_driver_and_lists_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let home = tmp.path().join("home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();

    // Seed a devin store the driver will open.
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

    let node = sepia_testkit::ensure_driver_bin("sepia-node");
    let output = std::process::Command::new(node)
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &home)
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sepia-node exited {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );
    assert!(stdout.contains("[devin]"), "stdout: {stdout}");
    assert!(stdout.contains("1 session(s)"), "stdout: {stdout}");
}
