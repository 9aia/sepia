#![allow(clippy::unwrap_used, clippy::pedantic)]

//! The hub's node credential store — file backend round-trips, on-disk
//! permissions, and backend selection. The real Secret Service can't be
//! forced in CI, so keyring selection is asserted against the live
//! probe result rather than a fixed backend.

use std::collections::HashMap;

use sepia_hub::credentials::{
    self, FileCredentials, KeyringCredentials, NodeCredentials, STORE_ENV,
};

fn lookup(vars: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
    |k| vars.get(k).cloned()
}

#[test]
fn file_store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileCredentials::new(path.clone());
    assert_eq!(store.get("n1").unwrap(), None);

    store.set("n1", "tok-1").unwrap();
    store.set("n2", "tok-2").unwrap();

    // A fresh instance reads the same file — persistence, not memory.
    let reopened = FileCredentials::new(path);
    assert_eq!(reopened.get("n1").unwrap().as_deref(), Some("tok-1"));
    assert_eq!(reopened.get("n2").unwrap().as_deref(), Some("tok-2"));

    reopened.set("n1", "tok-3").unwrap();
    assert_eq!(store.get("n1").unwrap().as_deref(), Some("tok-3"));

    // Delete is idempotent.
    store.delete("n1").unwrap();
    store.delete("n1").unwrap();
    assert_eq!(store.get("n1").unwrap(), None);
}

#[cfg(unix)]
#[test]
fn file_store_is_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileCredentials::new(path.clone());
    store.set("n1", "tok").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials.json must not be world-readable");
}

#[test]
fn file_store_surfaces_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::write(&path, "not json").unwrap();
    let store = FileCredentials::new(path.clone());
    assert!(store.get("n1").is_err());

    // Non-object JSON fails too; a missing file reads as empty.
    std::fs::write(&path, "[1,2]").unwrap();
    assert!(store.get("n1").is_err());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(store.get("n1").unwrap(), None);
}

#[test]
fn selection_honors_forced_backends() {
    let dir = tempfile::tempdir().unwrap();

    let vars = HashMap::from([(STORE_ENV.to_string(), "file".to_string())]);
    let store = credentials::from_lookup(dir.path(), lookup(&vars));
    assert_eq!(store.backend(), "file");

    // Forced keyring returns the keyring even when the probe fails —
    // explicit config; per-call errors surface honestly at use.
    let vars = HashMap::from([(STORE_ENV.to_string(), "keyring".to_string())]);
    let store = credentials::from_lookup(dir.path(), lookup(&vars));
    assert_eq!(store.backend(), "os-keyring");
}

#[test]
fn auto_picks_keyring_when_reachable_file_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let want = if credentials::keyring_probe().is_ok() {
        "os-keyring"
    } else {
        "file"
    };
    for (name, vars) in [
        ("unset", HashMap::new()),
        (
            "auto",
            HashMap::from([(STORE_ENV.to_string(), "auto".into())]),
        ),
        (
            "empty",
            HashMap::from([(STORE_ENV.to_string(), " ".into())]),
        ),
        (
            "bogus",
            HashMap::from([(STORE_ENV.to_string(), "bogus".into())]),
        ),
    ] {
        let store = credentials::from_lookup(dir.path(), lookup(&vars));
        assert_eq!(store.backend(), want, "{name} should behave like auto");
    }
}

#[test]
fn keyring_store_shape() {
    // No real Secret Service is guaranteed — only that the impl routes
    // through the trait and reports its backend. A working probe means
    // a set/get round-trip is also safe to assert.
    let store = KeyringCredentials;
    assert_eq!(store.backend(), "os-keyring");
    if credentials::keyring_probe().is_err() {
        return;
    }
    store.set("sepia-hub-probe", "tok").unwrap();
    assert_eq!(
        store.get("sepia-hub-probe").unwrap().as_deref(),
        Some("tok")
    );
    store.delete("sepia-hub-probe").unwrap();
    assert_eq!(store.get("sepia-hub-probe").unwrap(), None);
}
