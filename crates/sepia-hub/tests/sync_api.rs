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
