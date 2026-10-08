//! `sepia-driver-devin` — the Devin `sessions.db` driver binary.
//!
//! `--manifest` prints the manifest; with no flags it serves the
//! driver-wire protocol on stdio. The store path comes from
//! `SEPIA_DEVIN_DB` (default `~/.local/share/devin/cli/sessions.db`) and
//! `SEPIA_DEVIN_READONLY=1` opens read-only.

use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver_devin::DevinStore;
use sepia_driver_sdk::{Capability, DRIVER_PROTOCOL, DriverManifest, serve_store};

fn db_path() -> PathBuf {
    if let Ok(path) = std::env::var("SEPIA_DEVIN_DB") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/devin/cli/sessions.db")
}

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "devin".into(),
        label: "Devin sessions.db".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: [
            Capability::SessionStore,
            Capability::SessionWrite,
            Capability::Checkpoints,
            Capability::Rewind,
        ]
        .into_iter()
        .collect(),
        agent_command: Some("devin".into()),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "db": { "type": "string", "description": "sessions.db path" },
                "readonly": { "type": "boolean" }
            }
        }),
        backend_type: Some("windsurf".into()),
    }
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--manifest") {
        println!("{}", serde_json::to_string(&manifest()).unwrap_or_default());
        return Ok(());
    }
    let readonly = std::env::var("SEPIA_DEVIN_READONLY").is_ok_and(|v| v == "1" || v == "true");
    let store = DevinStore::open(&db_path(), readonly)
        .map_err(|e| std::io::Error::other(e.message.clone()))?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve_store(manifest(), Arc::new(store)))
}
