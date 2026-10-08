#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_meta::MetaStore;
use sepia_push::{PushPrefs, PushStore, PushSubscription, SubscriptionKeys};
use serde_json::json;

fn store() -> (PushStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    (PushStore::open(meta), dir)
}

fn sub(endpoint: &str) -> PushSubscription {
    PushSubscription {
        endpoint: endpoint.into(),
        keys: SubscriptionKeys {
            auth: "auth".into(),
            p256dh: "p256dh".into(),
        },
        prefs: PushPrefs::default(),
    }
}

#[test]
fn vapid_keys_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaStore::open(&dir.path().join("meta.json"));
    let a = PushStore::open(meta.clone());
    let b = PushStore::open(meta);
    assert_eq!(a.public_key(), b.public_key());
    assert!(!a.public_key().is_empty());
}

#[test]
fn upsert_replaces_by_endpoint_and_remove_works() {
    let (push, _dir) = store();
    push.upsert(sub("https://push.example/1"));
    push.upsert(sub("https://push.example/2"));
    let mut replaced = sub("https://push.example/1");
    replaced.prefs.done = false;
    push.upsert(replaced);
    assert_eq!(push.list().len(), 2);
    assert!(
        !push
            .list()
            .iter()
            .find(|s| s.endpoint.ends_with("/1"))
            .unwrap()
            .prefs
            .done
    );
    push.remove("https://push.example/1");
    assert_eq!(push.list().len(), 1);
}

#[test]
fn subscriptions_persist_to_meta_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.json");
    let meta = MetaStore::open(&path);
    let push = PushStore::open(meta.clone());
    push.upsert(sub("https://push.example/1"));
    // Reopen — the config blob round-trips.
    let meta2 = MetaStore::open(&path);
    let push2 = PushStore::open(meta2);
    assert_eq!(push2.list().len(), 1);
    // And the raw file carries the key.
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        raw["config"]["pushSubscriptions"][0]["endpoint"],
        json!("https://push.example/1")
    );
}

#[test]
fn send_is_a_noop_with_no_matching_prefs() {
    let (push, _dir) = store();
    let mut s = sub("https://push.example/none");
    s.prefs.permission = false;
    push.upsert(s);
    // No recipients → nothing sent (and no panic without a live endpoint).
    assert_eq!(push.send(sepia_push::Kind::Permission, "t", "b", "/"), 0);
}
