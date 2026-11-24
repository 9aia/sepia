//! Node-local key/value surface (org proposal P-006 phase 2) — the
//! generic half is `hypha-kv`'s `{ns}/{key}` store behind hypha-http's
//! `/api/kv*` routes; this module is the Peregrin integration: which
//! namespaces mean what, the compat `/api/projects*` + `/api/config*`
//! mapping, the feed-event contract, the project-delete cascade, and
//! the one-shot meta.json → KV migration.
//!
//! Namespace layout:
//!
//! | ns        | visibility | contents                                  |
//! |-----------|------------|-------------------------------------------|
//! | `config`  | public     | app/UI config (the old meta `config` map)  |
//! | `project` | public     | `{id → {"name": …}}` project records       |
//! | `push`    | internal   | `vapid` + `pushSubscriptions` — store-API only, `/api/kv*` hides it |
//! | `migration` | internal | the `meta-json` import marker             |
//!
//! `sessions` stay in sepia-meta's `meta.json` — only `projects` and
//! `config` moved (sepia-meta is sessions-only since phase 3; legacy
//! `projects`/`config` keys in meta.json are ignored on read and dropped
//! on the next flush). `INTERNAL_CONFIG` survives as [`INTERNAL_CONFIG`]:
//! the compat `/api/config*` routes keep rejecting those keys so the
//! public `config` namespace can't be poisoned through the old wire;
//! the generic `/api/kv*` surface enforces the same boundary via the
//! `push` namespace's internal visibility instead of a key denylist.
//!
//! Secrets policy (proposal open q4): VAPID private material +
//! push subscriptions are secret *references* at most — they live in
//! the internal `push` namespace, never in public `config`. Moving
//! them to cuora is a phase-3 follow-up, not done here.

use std::collections::BTreeMap;
use std::sync::Arc;

use hypha_http::{KvHook, KvWrite};
use hypha_kv::{KvStore, Visibility};
use sepia_meta::{MetaStore, SessionMeta};
use serde::Serialize;
use serde_json::{Value, json};

/// Compat `GET /api/projects` row — `{id, name}` (was
/// `sepia_meta::Project` before the sepia-meta shrink).
#[derive(Clone, Debug, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

/// Public app/UI config — the old meta.json `config` section.
pub const NS_CONFIG: &str = "config";
/// Public project records — `{id → {"name": …}}`.
pub const NS_PROJECT: &str = "project";
/// Internal push secrets — `vapid` + `pushSubscriptions`. Declared
/// [`Visibility::Internal`] at boot so `/api/kv*` never serves it;
/// store-API callers (the push backend, migration) read/write freely.
pub const NS_PUSH: &str = "push";
/// Internal one-shot migration marker namespace.
const NS_MIGRATION: &str = "migration";
/// Marker key — its presence means the meta.json import already ran.
const MIGRATION_KEY: &str = "meta-json";

/// Config keys that must never sit in the public `config` namespace —
/// the old `INTERNAL_CONFIG` denylist, now enforced twice: the compat
/// `/api/config*` routes reject them (400) and their real home is the
/// internal [`NS_PUSH`] namespace.
pub const INTERNAL_CONFIG: [&str; 2] = ["vapid", "pushSubscriptions"];

/// Tolerant project-name read — our stored shape is `{"name": …}`;
/// a bare JSON string is accepted too (hand-written `/api/kv` values).
fn project_name(value: &Value) -> Option<String> {
    value
        .as_object()
        .and_then(|o| o.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.as_str().map(str::to_string))
}

/// The compat `GET /api/projects` row list — `kv.list` is a
/// `BTreeMap`, so ids come back sorted like the old meta map did.
/// Entries without a recoverable name are skipped (they can't happen
/// through the compat routes; only hand-written `/api/kv` values).
#[must_use]
pub fn list_projects(kv: &dyn KvStore) -> Vec<Project> {
    kv.list(NS_PROJECT)
        .into_iter()
        .filter_map(|(id, value)| project_name(&value).map(|name| Project { id, name }))
        .collect()
}

/// `MetaStore::ensure_project` over KV — create under a caller-chosen
/// id or refresh the name; returns the project plus whether the name
/// actually changed (the transfer path's emit suppression). Existing
/// object fields other than `name` are preserved.
#[must_use]
pub fn ensure_project(kv: &dyn KvStore, id: &str, name: &str) -> (Project, bool) {
    let changed =
        project_name(&kv.get(NS_PROJECT, id).unwrap_or(Value::Null)).as_deref() != Some(name);
    if changed {
        let mut value = kv
            .get(NS_PROJECT, id)
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        value.insert("name".into(), Value::String(name.to_string()));
        kv.set(NS_PROJECT, id, Value::Object(value));
    }
    (
        Project {
            id: id.to_string(),
            name: name.to_string(),
        },
        changed,
    )
}

/// A fresh project id — the `proj_<8 hex>` shape `MetaStore` minted.
#[must_use]
pub fn new_project_id() -> String {
    format!("proj_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// The `{"id","patch"}` project feed payload — the shape
/// `InstrumentedMeta` emitted for project writes. `value` is the
/// stored KV value verbatim; a recognizable `name` keeps the old
/// `{name}` patch shape, anything else rides as the raw patch.
#[must_use]
pub fn project_set_payload(id: &str, value: &Value) -> Value {
    match project_name(value) {
        Some(name) => json!({ "id": id, "patch": { "name": name } }),
        None => json!({ "id": id, "patch": value }),
    }
}

/// The project-delete feed payload (`{"deleted": true}`).
#[must_use]
pub fn project_delete_payload(id: &str) -> Value {
    json!({ "id": id, "patch": { "deleted": true } })
}

/// Strip `project_id` from every session meta's `projectIds` — the
/// `MetaStore::delete_project` cascade, re-implemented because the
/// project record now lives in KV while session metas stay in
/// sepia-meta. Writes go through `MetaStore::patch` directly (silent —
/// the old cascade emitted no `meta` events either).
pub fn strip_project_from_sessions(meta: &MetaStore, project_id: &str) {
    for (id, session) in meta.sessions() {
        let Some(ids) = &session.project_ids else {
            continue;
        };
        if !ids.iter().any(|p| p == project_id) {
            continue;
        }
        let kept: Vec<String> = ids
            .iter()
            .filter(|p| p.as_str() != project_id)
            .cloned()
            .collect();
        let patch = SessionMeta {
            project_ids: Some(kept),
            ..SessionMeta::default()
        };
        meta.patch(&id, &patch);
    }
}

/// The `HyphaState::kv_hook` — maps `/api/kv*` writes onto Peregrin's
/// feed contract: `project`-namespace writes emit `project` events in
/// the shapes `InstrumentedMeta` used (set → `{name}` patch, delete →
/// `{deleted: true}`), and project deletes cascade-strip the id from
/// session metas. Every other namespace is silent — config writes
/// historically emitted nothing, and internal namespaces can't be
/// written over HTTP anyway. Compat-route writes bypass this hook
/// (they're store-API calls) and emit/cascade themselves.
pub fn write_hook(meta: Option<crate::feed::InstrumentedMeta>) -> KvHook {
    Arc::new(move |write: &KvWrite, feed: &hypha_http::EventFeed| {
        if write.ns != NS_PROJECT {
            return;
        }
        match &write.op {
            hypha_kv::KvOp::Set { value } => {
                feed.emit("project", project_set_payload(&write.key, value));
            }
            hypha_kv::KvOp::Delete => {
                if let Some(meta) = &meta {
                    strip_project_from_sessions(meta.store(), &write.key);
                }
                feed.emit("project", project_delete_payload(&write.key));
            }
        }
    })
}

/// One-shot meta.json → KV import, run at `SessionState::from_env` boot.
/// Guarded by a marker key in the internal `migration` namespace.
///
/// Reads the legacy `projects`/`config` sections straight off the
/// meta.json file: sepia-meta is sessions-only since phase 3 and
/// drops those keys on its next flush, so the import must run before
/// (and independently of) the overlay store — a pre-phase-2 install
/// upgrading straight here still gets its data across.
///
/// Per-section guards (`kv.list(ns).is_empty()`) keep a partially
/// populated store honest on the first run; `INTERNAL_CONFIG` keys in
/// the meta config map land in the internal `push` namespace, never
/// in public `config`.
pub fn migrate_meta(meta_path: &std::path::Path, kv: &Arc<dyn KvStore>) {
    // Idempotent declarations — a hand-edited kv.json can't publish
    // the internal namespaces by dropping their visibility.
    kv.declare(NS_PUSH, Visibility::Internal);
    kv.declare(NS_MIGRATION, Visibility::Internal);
    if kv.get(NS_MIGRATION, MIGRATION_KEY).is_some() {
        return;
    }

    // v2 meta.json is `{sessions, projects, config}`; anything else
    // (v1 flat session map, missing/corrupt file) has no sections to
    // import — the marker still lands so we don't re-scan every boot.
    let record = std::fs::read_to_string(meta_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    if let Some(projects) = record.get("projects").and_then(Value::as_object) {
        if !projects.is_empty() && kv.list(NS_PROJECT).is_empty() {
            for (id, value) in projects {
                if let Some(name) = project_name(value) {
                    kv.set(NS_PROJECT, id, json!({ "name": name }));
                }
            }
        }
    }

    if let Some(config) = record.get("config").and_then(Value::as_object) {
        if !config.is_empty() {
            let mut public: BTreeMap<String, Value> = BTreeMap::new();
            for (key, value) in config {
                if INTERNAL_CONFIG.contains(&key.as_str()) {
                    if kv.get(NS_PUSH, key).is_none() {
                        kv.set(NS_PUSH, key, value.clone());
                    }
                } else {
                    public.insert(key.clone(), value.clone());
                }
            }
            if !public.is_empty() && kv.list(NS_CONFIG).is_empty() {
                for (key, value) in public {
                    kv.set(NS_CONFIG, &key, value);
                }
            }
        }
    }

    kv.set(
        NS_MIGRATION,
        MIGRATION_KEY,
        json!({ "version": 1, "from": "meta.json" }),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::assert_is_empty)]
    use super::*;
    use sepia_meta::SessionMeta;

    fn temp_kv() -> (tempfile::TempDir, Arc<dyn KvStore>) {
        let dir = tempfile::tempdir().unwrap();
        let kv: Arc<dyn KvStore> = Arc::new(hypha_kv_file::JsonKvStore::open(
            &dir.path().join("kv.json"),
        ));
        (dir, kv)
    }

    #[test]
    fn project_roundtrip_and_list_order() {
        let (_dir, kv) = temp_kv();
        assert!(list_projects(&*kv).is_empty());
        let (p, changed) = ensure_project(&*kv, "proj_b", "Beta");
        assert!(changed);
        assert_eq!(p.name, "Beta");
        let (p2, changed2) = ensure_project(&*kv, "proj_a", "Alpha");
        assert!(changed2);
        // Same name → no change reported.
        let (_, changed3) = ensure_project(&*kv, "proj_a", "Alpha");
        assert!(!changed3);
        // Rename preserves sibling fields.
        kv.set("project", "proj_a", json!({"name": "Alpha", "extra": 1}));
        let _ = ensure_project(&*kv, "proj_a", "Alpha2");
        assert_eq!(kv.get("project", "proj_a").unwrap()["extra"], json!(1));
        let list = list_projects(&*kv);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "proj_a"); // BTreeMap order
        assert_eq!(list[1].id, "proj_b");
        let _ = p2;
    }

    #[test]
    fn project_name_tolerates_string_and_object() {
        assert_eq!(project_name(&json!({"name": "X"})).as_deref(), Some("X"));
        assert_eq!(project_name(&json!("Y")).as_deref(), Some("Y"));
        assert_eq!(project_name(&json!({"other": 1})), None);
        assert_eq!(project_name(&json!(null)), None);
    }

    #[test]
    fn set_payload_shapes() {
        assert_eq!(
            project_set_payload("p1", &json!({"name": "Web"})),
            json!({"id": "p1", "patch": {"name": "Web"}})
        );
        // Non-name values ride as the raw patch.
        assert_eq!(
            project_set_payload("p1", &json!([1, 2])),
            json!({"id": "p1", "patch": [1, 2]})
        );
        assert_eq!(
            project_delete_payload("p1"),
            json!({"id": "p1", "patch": {"deleted": true}})
        );
    }

    #[test]
    fn strip_project_cascades_into_session_meta() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaStore::open(&dir.path().join("meta.json"));
        let patch = SessionMeta {
            project_ids: Some(vec!["p1".into(), "p2".into()]),
            ..SessionMeta::default()
        };
        meta.patch("s1", &patch);
        let patch2 = SessionMeta {
            project_ids: Some(vec!["p2".into()]),
            ..SessionMeta::default()
        };
        meta.patch("s2", &patch2);
        strip_project_from_sessions(&meta, "p1");
        assert_eq!(
            meta.of("s1").unwrap().project_ids.unwrap(),
            vec!["p2".to_string()]
        );
        // Untouched session keeps its ids.
        assert_eq!(
            meta.of("s2").unwrap().project_ids.unwrap(),
            vec!["p2".to_string()]
        );
        // Stripping the last id leaves Some([]) — the old retain() shape.
        strip_project_from_sessions(&meta, "p2");
        assert_eq!(
            meta.of("s1").unwrap().project_ids,
            Some(Vec::<String>::new())
        );
    }

    /// Write a legacy v2 meta.json — sepia-meta no longer stores
    /// projects/config, so tests seed the file shape directly.
    fn write_legacy_meta(path: &std::path::Path, projects: &Value, config: &Value) {
        std::fs::write(
            path,
            serde_json::json!({
                "sessions": {"s1": {"title": "T"}},
                "projects": projects,
                "config": config,
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn migrate_meta_imports_once_and_splits_internal_keys() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join("meta.json");
        write_legacy_meta(
            &meta_path,
            &json!({"proj_1": {"name": "Web"}}),
            &json!({
                "theme": "dark",
                "vapid": {"private": "x"},
                "pushSubscriptions": [],
            }),
        );
        let kv: Arc<dyn KvStore> = Arc::new(hypha_kv_file::JsonKvStore::open(
            &dir.path().join("kv.json"),
        ));
        migrate_meta(&meta_path, &kv);

        assert_eq!(kv.visibility("push"), Visibility::Internal);
        assert_eq!(kv.visibility("migration"), Visibility::Internal);
        assert_eq!(kv.get("project", "proj_1"), Some(json!({"name": "Web"})));
        assert_eq!(kv.get("config", "theme"), Some(json!("dark")));
        // Internal keys never touched public config.
        assert!(kv.get("config", "vapid").is_none());
        assert_eq!(kv.get("push", "vapid"), Some(json!({"private": "x"})));
        assert_eq!(kv.get("push", "pushSubscriptions"), Some(json!([])));

        // Second boot: marker present → nothing re-imports even if the
        // project was deleted from KV (meta.json still carries it).
        kv.delete("project", "proj_1");
        migrate_meta(&meta_path, &kv);
        assert!(kv.get("project", "proj_1").is_none());
    }

    #[test]
    fn migrate_meta_skips_nonempty_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join("meta.json");
        write_legacy_meta(
            &meta_path,
            &json!({"proj_old": {"name": "Old"}}),
            &json!({"theme": "dark"}),
        );
        let kv: Arc<dyn KvStore> = Arc::new(hypha_kv_file::JsonKvStore::open(
            &dir.path().join("kv.json"),
        ));
        // Pre-populated namespaces win over the import.
        kv.set("project", "proj_new", json!({"name": "New"}));
        kv.set("config", "theme", json!("light"));
        migrate_meta(&meta_path, &kv);
        assert!(kv.get("project", "proj_old").is_none());
        assert_eq!(kv.get("config", "theme"), Some(json!("light")));
        // The marker still landed.
        assert!(kv.get("migration", "meta-json").is_some());
    }

    #[test]
    fn new_project_id_shape() {
        let id = new_project_id();
        assert!(id.starts_with("proj_"));
        assert_eq!(id.len(), "proj_".len() + 8);
    }
}
