//! Stable per-machine identity — mirrors `apps/server/src/node.ts`: the id is
//! minted on first boot and persisted at `$SEPIA_HOME/node.json`; afterwards
//! read back, never regenerated. Writes are atomic (tmp + rename).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const SEPIA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sepia protocol revision — bump on breaking changes (docs/protocol.md).
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub version: String,
}

fn mint_id() -> String {
    use ring::rand::SecureRandom;
    use std::fmt::Write;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .unwrap_or_else(|_| panic!("system RNG unavailable"));
    let mut hex = String::with_capacity(32);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    format!("node_{}", &hex[..16])
}

#[must_use]
pub fn load_node_identity(path: &Path, name: &str) -> NodeIdentity {
    if path.exists()
        && let Ok(raw) = fs::read_to_string(path)
        && let Ok(record) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(id) = record
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    {
        let stored_name = record
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        return NodeIdentity {
            id: id.to_string(),
            name: stored_name.unwrap_or(name).to_string(),
            version: SEPIA_VERSION.to_string(),
        };
    }
    // A corrupt file falls through to a fresh identity — boot must not crash.

    let identity = NodeIdentity {
        id: mint_id(),
        name: name.to_string(),
        version: SEPIA_VERSION.to_string(),
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    if fs::write(
        &tmp,
        serde_json::json!({ "id": identity.id, "name": identity.name }).to_string(),
    )
    .is_ok()
    {
        let _ = fs::rename(&tmp, path);
    }
    identity
}

/// PATCH /api/node — rename: persists `name`, keeps the id.
pub fn rename_node(path: &Path, name: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let id = load_node_identity(path, name).id;
    fs::write(
        &tmp,
        serde_json::json!({ "id": id, "name": name }).to_string(),
    )?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_and_persists_identity() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("node.json");
        let first = load_node_identity(&path, "box");
        assert!(first.id.starts_with("node_"));
        assert_eq!(first.name, "box");
        // Second load reads back the same id.
        let second = load_node_identity(&path, "box");
        assert_eq!(first.id, second.id);
    }

    #[test]
    fn corrupt_file_mints_fresh() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("node.json");
        fs::write(&path, "not json").unwrap_or_else(|e| panic!("{e}"));
        let id = load_node_identity(&path, "box");
        assert!(id.id.starts_with("node_"));
    }

    #[test]
    fn stored_name_wins_over_passed_name() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("node.json");
        fs::write(&path, r#"{"id":"node_x","name":"old"}"#).unwrap_or_else(|e| panic!("{e}"));
        let id = load_node_identity(&path, "new");
        assert_eq!(id.name, "old");
    }

    #[test]
    fn rename_keeps_id() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("node.json");
        let first = load_node_identity(&path, "box");
        rename_node(&path, "renamed").unwrap_or_else(|e| panic!("{e}"));
        let after = load_node_identity(&path, "box");
        assert_eq!(after.id, first.id);
        assert_eq!(after.name, "renamed");
    }
}
