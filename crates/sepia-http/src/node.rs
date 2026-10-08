//! Stable per-machine node identity — port of `apps/server/src/node.ts`.
//! The id is minted on first boot and persisted to `$SEPIA_HOME/node.json`;
//! afterwards it is read back, never regenerated.

use std::path::Path;

use serde::Serialize;

/// Sepia protocol revision — bump on breaking changes (docs/protocol.md).
pub const PROTOCOL_VERSION: u32 = 1;

/// What `GET /api/node` reports; `version` defaults to the crate stamp.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeIdentity {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// The package version stamp — the same value `GET /api/node` reports.
pub const SEPIA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Mint the `node_<hex16>` id shape TS uses.
fn mint_id() -> String {
    format!("node_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

/// Persist `{id, name}` atomically (tmp + rename) — the same write
/// `load_node_identity` performs on first boot and `PATCH /api/node`
/// on rename.
///
/// # Errors
/// `std::io::Error` when the file cannot be written or renamed.
pub fn save_node_identity(path: &Path, identity: &NodeIdentity) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let json = serde_json::to_string(&serde_json::json!({
        "id": identity.id,
        "name": identity.name,
    }))
    .map_err(std::io::Error::other)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// Load or mint the node identity. A corrupt file regenerates rather
/// than crashing; the write is atomic (tmp + rename).
pub fn load_node_identity(path: &Path, name: &str) -> NodeIdentity {
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(serde_json::Value::Object(record)) = serde_json::from_str(&raw) {
            let id = record.get("id").and_then(|v| v.as_str());
            if let Some(id) = id.filter(|i| !i.is_empty()) {
                let name = record
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(name);
                return NodeIdentity {
                    id: id.to_string(),
                    name: name.to_string(),
                    version: SEPIA_VERSION.to_string(),
                };
            }
        }
        // A corrupt or id-less file falls through to a fresh identity.
    }
    let identity = NodeIdentity {
        id: mint_id(),
        name: name.to_string(),
        version: SEPIA_VERSION.to_string(),
    };
    let _ = save_node_identity(path, &identity);
    identity
}
