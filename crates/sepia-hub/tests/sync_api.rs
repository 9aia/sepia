#![allow(clippy::unwrap_used, clippy::pedantic)]

//! `SyncNodeApi` — projection-backed list/get, owning-node history,
//! queued writes when the node is down.

use sepia_hub::sync_api::{HubNode, SyncNodeApi, parse_nodes};
use sepia_sync::{NodeTarget, SyncEngine, SyncHandle};
use sepia_web::api::NodeApi;
use serde_json::json;

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
    let api = SyncNodeApi::new(engine, nodes);
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
    let api = SyncNodeApi::new(engine, nodes);
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
    let api = SyncNodeApi::new(engine, nodes);
    api.prompt("s1", None, "hello offline").await.unwrap();
    // The queue holds the write for the session.
    let pending = outbox.pending_for_node("laptop").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id, "s1");
    assert_eq!(pending[0].op, "prompt");
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
        .block_on(sepia_core::storage::SessionRepository::save(&store, &session))
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
        .env("SEPIA_NODES", format!("laptop=http://127.0.0.1:{node_port}"))
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
        match client
            .get(format!("http://127.0.0.1:{hub_port}/"))
            .call()
        {
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
