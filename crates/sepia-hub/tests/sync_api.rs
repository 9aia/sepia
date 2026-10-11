#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `SyncNodeApi` — projection-backed list/get, owning-node history,
//! queued writes when the node is down.

use sepia_hub::credentials::{FileCredentials, NodeCredentials};
use sepia_hub::sync_api::{HubNode, SyncNodeApi, parse_nodes};
use sepia_sync::{NodeTarget, SyncEngine, SyncHandle};
use sepia_web::api::NodeApi;
use serde_json::json;
use std::sync::Arc;

/// A file-backed credential store in a temp dir — the `TempDir` keeps
/// the path alive for the test's duration.
fn file_creds() -> (tempfile::TempDir, Arc<dyn NodeCredentials>) {
    let dir = tempfile::tempdir().unwrap();
    let creds = Arc::new(FileCredentials::new(dir.path().join("credentials.json")));
    (dir, creds)
}

fn engine_with(
    nodes: Vec<HubNode>,
) -> (
    SyncHandle,
    sepia_sync::ProjectionStore,
    sepia_outbox::Outbox,
) {
    let projection = sepia_sync::ProjectionStore::in_memory().unwrap();
    let outbox = sepia_outbox::Outbox::in_memory().unwrap();
    let targets = nodes.iter().map(|n| {
        NodeTarget::new(sepia_sync::NodeRef {
            id: n.id.clone(),
            url: n.url.clone(),
            label: n.id.clone(),
        })
    });
    (
        SyncEngine::spawn(
            projection.clone(),
            outbox.clone(),
            targets,
            Default::default(),
        ),
        projection,
        outbox,
    )
}

fn node(id: &str, url: &str) -> HubNode {
    HubNode {
        id: id.into(),
        url: url.into(),
        token: None,
    }
}

fn seed(projection: &sepia_sync::ProjectionStore) {
    projection
        .upsert_summary(
            "laptop",
            &json!({
                "id": "s1", "title": "Alpha", "cwd": "/work",
                "agent": "devin", "source": "devin",
                "updatedAt": "2024-01-01T00:00:00Z",
            }),
        )
        .unwrap();
}

#[test]
fn parses_node_lists() {
    let nodes = parse_nodes("a=http://10.0.0.1:8787; b = http://laptop:8787@secret ; ");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].id, "a");
    assert_eq!(nodes[0].url, "http://10.0.0.1:8787");
    assert_eq!(nodes[0].token, None);
    assert_eq!(nodes[1].token.as_deref(), Some("secret"));
    assert_eq!(nodes[1].url, "http://laptop:8787");
}

#[tokio::test]
async fn list_reads_the_projection_across_nodes() {
    let nodes = vec![
        node("laptop", "http://127.0.0.1:9"),
        node("tower", "http://127.0.0.1:9"),
    ];
    let (engine, projection, _outbox) = engine_with(nodes.clone());
    seed(&projection);
    projection
        .upsert_summary(
            "tower",
            &json!({
                "id": "s2", "title": "Beta", "agent": "cline",
                "source": "cline", "locked": true,
            }),
        )
        .unwrap();
    let (_creds_dir, creds) = file_creds();
    let api = SyncNodeApi::new(engine, nodes, creds);
    let list = api.list_sessions().await.unwrap();
    assert_eq!(list.len(), 2);
    let titles: Vec<&str> = list.iter().map(|s| s.title.as_str()).collect();
    assert!(titles.contains(&"Alpha") && titles.contains(&"Beta"));
    // The owning node rides along for the UI.
    let beta = list.iter().find(|s| s.id == "s2").unwrap();
    assert_eq!(serde_json::to_value(beta).unwrap()["node"], "tower");
    // The test runtime drop aborts the engine loops.
}

#[tokio::test]
async fn get_session_returns_the_raw_wire_summary() {
    let nodes = vec![node("laptop", "http://127.0.0.1:9")];
    let (engine, projection, _outbox) = engine_with(nodes.clone());
    seed(&projection);
    let (_creds_dir, creds) = file_creds();
    let api = SyncNodeApi::new(engine, nodes, creds);
    let s = api.get_session("s1", None).await.unwrap();
    assert_eq!(s.title, "Alpha");
    assert!(api.get_session("missing", None).await.is_err());
    // The test runtime drop aborts the engine loops.
}

#[tokio::test]
async fn prompt_queues_when_the_node_is_down() {
    // Port 9 = discard protocol — nothing listens, so the write queues.
    let nodes = vec![node("laptop", "http://127.0.0.1:9")];
    let (engine, projection, outbox) = engine_with(nodes.clone());
    seed(&projection);
    let (_creds_dir, creds) = file_creds();
    let api = SyncNodeApi::new(engine, nodes, creds);
    api.prompt("s1", None, "hello offline", &[]).await.unwrap();
    // The queue holds the write for the session.
    let pending = outbox.pending_for_node("laptop").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id, "s1");
    assert_eq!(pending[0].op, "prompt");
    // The test runtime drop aborts the engine loops.
}

#[tokio::test]
async fn pending_writes_reports_queued_and_dead() {
    let nodes = vec![node("laptop", "http://127.0.0.1:9")];
    let (engine, projection, outbox) = engine_with(nodes.clone());
    seed(&projection);
    let (_creds_dir, creds) = file_creds();
    let api = SyncNodeApi::new(engine, nodes, creds);

    // One queued write via the API, one forced dead-letter straight
    // into the outbox (a `turn` op dies on its first failure).
    api.prompt("s1", None, "hello offline", &[]).await.unwrap();
    let dead = outbox
        .enqueue(
            "laptop",
            "s1",
            "cancel",
            sepia_outbox::OpKind::Turn,
            json!({}),
            "dead-key",
            None,
        )
        .unwrap();
    assert_eq!(
        outbox.mark_failed(&dead.id, "node refused").unwrap(),
        sepia_outbox::Status::Dead
    );

    let writes = api.pending_writes().await.unwrap();
    assert_eq!(writes.len(), 2);
    let queued = writes.iter().find(|w| w.op == "prompt").unwrap();
    assert_eq!(queued.status, "queued");
    assert_eq!(queued.node_id, "laptop");
    assert_eq!(queued.session_id, "s1");
    assert_eq!(queued.kind, "turn");
    let failed = writes.iter().find(|w| w.op == "cancel").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.last_error.as_deref(), Some("node refused"));
    // The test runtime drop aborts the engine loops.
}

/// The whole stack: sepia-node (devin driver + mock agent) serving,
/// sepia-hub projecting it, GET / SSRs the session title.
#[test]
fn hub_e2e_over_a_real_node() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let node_home = tmp.path().join("node-home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&node_home).unwrap();

    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();
    let db = db_dir.join("sessions.db");
    let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
    let mut session =
        sepia_testkit::contract::session("e2e-1", "Capstone session", 1_700_000_000.0);
    session.backend_type = "windsurf".into();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sepia_core::storage::SessionRepository::save(
            &store, &session,
        ))
        .unwrap();

    let node_port = 18790u16;
    let mut node = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &node_home)
        .env("SEPIA_META", node_home.join("meta.json"))
        .env("SEPIA_NODE", node_home.join("node.json"))
        .env("SEPIA_PORT", node_port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let hub_port = 18791u16;
    let mut hub = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-hub"))
        .env("SEPIA_NODE_URL", format!("http://127.0.0.1:{node_port}"))
        .env(
            "SEPIA_NODES",
            format!("laptop=http://127.0.0.1:{node_port}"),
        )
        .env("SEPIA_HOME", tmp.path().join("hub-home"))
        .env("SEPIA_HUB_PORT", hub_port.to_string())
        .env("SEPIA_HUB_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let client = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let body = loop {
        match client.get(format!("http://127.0.0.1:{hub_port}/")).call() {
            Ok(mut resp) => {
                let text = resp.body_mut().read_to_string().unwrap();
                if text.contains("Capstone session") {
                    break text;
                }
                // SSR may render before the projection's first sync —
                // keep polling until it lands.
                if std::time::Instant::now() >= deadline {
                    break text;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e) => {
                let _ = hub.kill();
                let _ = node.kill();
                panic!("hub never came up: {e}");
            }
        }
    };
    let _ = hub.kill();
    let _ = node.kill();
    let _ = hub.wait();
    let _ = node.wait();
    assert!(body.contains("Capstone session"), "html: {body}");
}

/// Poll `cond` until it holds or `timeout` lapses (the hub tests have
/// no shared wait helper).
async fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// `pair` against a real `sepia-node` (SEPIA_TOKEN gated): the minted
/// credential lands in the hub's credential store, the node client is
/// rebuilt with it, and the engine's sync loop retargets — the node
/// comes up without a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_persists_and_adopts_the_minted_token() {
    let tmp = tempfile::tempdir().unwrap();
    let node_home = tmp.path().join("node-home");
    std::fs::create_dir_all(&node_home).unwrap();

    let node_port = 18794u16;
    let mut node = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_HOME", &node_home)
        .env("SEPIA_META", node_home.join("meta.json"))
        .env("SEPIA_NODE", node_home.join("node.json"))
        .env("SEPIA_PORT", node_port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("SEPIA_TOKEN", "static-tok")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Wait for health, then drop a one-time code where the node reads it.
    let url = format!("http://127.0.0.1:{node_port}");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(2)))
        .build()
        .new_agent();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match agent.get(format!("{url}/api/health")).call() {
            Ok(_) => break,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = node.kill();
                panic!("node never came up: {e}");
            }
        }
    }
    let code = sepia_http::pair::mint_pair_code();
    std::fs::write(
        node_home.join("pair-code"),
        serde_json::json!({ "code": code, "expiresAt": now_ms() + 60_000 }).to_string(),
    )
    .unwrap();

    let (_creds_dir, creds) = file_creds();
    let nodes = vec![HubNode {
        id: "n1".into(),
        url: url.clone(),
        token: None,
    }];
    let projection = sepia_sync::ProjectionStore::in_memory().unwrap();
    let engine = SyncEngine::spawn(
        projection.clone(),
        sepia_outbox::Outbox::in_memory().unwrap(),
        nodes.iter().map(|n| {
            NodeTarget::new(sepia_sync::NodeRef {
                id: n.id.clone(),
                url: n.url.clone(),
                label: n.id.clone(),
            })
        }),
        Default::default(),
    );
    let api = SyncNodeApi::new(engine, nodes, Arc::clone(&creds));

    // Tokenless: every gated call 401s.
    assert!(api.list_agents().await.is_err());

    let token = api.pair(&code, None).await.unwrap();
    assert!(token.starts_with("sepia_"), "token shape: {token}");
    assert_eq!(
        creds.get("n1").unwrap().as_deref(),
        Some(token.as_str()),
        "minted token persisted to the credential store"
    );

    // The rebuilt client authenticates; the retargeted sync loop
    // brings the node up without a restart.
    assert!(api.list_agents().await.is_ok());
    wait_for(
        || {
            projection
                .node("n1")
                .unwrap()
                .is_some_and(|n| n.status == sepia_sync::NodeStatus::Up)
        },
        "retargeted node reports up",
    )
    .await;

    let _ = node.kill();
    let _ = node.wait();
}

/// Boot-order resolution: a `SEPIA_NODES` entry without `@token` picks
/// the credential up from `$SEPIA_HOME/hub/credentials.json` (the file
/// backend, forced via SEPIA_CREDENTIAL_STORE=file).
#[test]
fn hub_resolves_node_tokens_from_the_credential_store() {
    let tmp = tempfile::tempdir().unwrap();
    let driver_dir = tmp.path().join("drivers");
    let node_home = tmp.path().join("node-home");
    let db_dir = tmp.path().join("devin");
    std::fs::create_dir_all(&driver_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&node_home).unwrap();

    let driver = sepia_testkit::ensure_driver_bin("sepia-driver-devin");
    std::fs::hard_link(&driver, driver_dir.join("sepia-driver-devin")).unwrap();
    let db = db_dir.join("sessions.db");
    let store = sepia_driver_devin::store::DevinStore::open(&db, false).unwrap();
    let session = sepia_testkit::contract::session("e2e-2", "Stored-cred session", 1_700_000_000.0);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sepia_core::storage::SessionRepository::save(
            &store, &session,
        ))
        .unwrap();

    let node_port = 18792u16;
    let mut node = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-node"))
        .env("SEPIA_DRIVER_DIR", &driver_dir)
        .env("SEPIA_DEVIN_DB", &db)
        .env("SEPIA_HOME", &node_home)
        .env("SEPIA_META", node_home.join("meta.json"))
        .env("SEPIA_NODE", node_home.join("node.json"))
        .env("SEPIA_PORT", node_port.to_string())
        .env("SEPIA_HOST", "127.0.0.1")
        .env("SEPIA_TOKEN", "stored-tok")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Preseed the file store — the hub reads it for tokenless nodes.
    let hub_home = tmp.path().join("hub-home");
    std::fs::create_dir_all(hub_home.join("hub")).unwrap();
    std::fs::write(
        hub_home.join("hub/credentials.json"),
        serde_json::json!({ "laptop": "stored-tok" }).to_string(),
    )
    .unwrap();

    let hub_port = 18793u16;
    let mut hub = std::process::Command::new(sepia_testkit::ensure_driver_bin("sepia-hub"))
        .env(
            "SEPIA_NODES",
            format!("laptop=http://127.0.0.1:{node_port}"),
        )
        .env("SEPIA_CREDENTIAL_STORE", "file")
        .env("SEPIA_HOME", &hub_home)
        .env("SEPIA_HUB_PORT", hub_port.to_string())
        .env("SEPIA_HUB_HOST", "127.0.0.1")
        .env("HOME", tmp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let client = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let body = loop {
        match client.get(format!("http://127.0.0.1:{hub_port}/")).call() {
            Ok(mut resp) => {
                let text = resp.body_mut().read_to_string().unwrap();
                if text.contains("Stored-cred session") {
                    break text;
                }
                if std::time::Instant::now() >= deadline {
                    break text;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e) => {
                let _ = hub.kill();
                let _ = node.kill();
                panic!("hub never came up: {e}");
            }
        }
    };
    let _ = hub.kill();
    let _ = node.kill();
    let _ = hub.wait();
    let _ = node.wait();
    assert!(
        body.contains("Stored-cred session"),
        "stored credential authenticated the sync — html: {body}"
    );
}
