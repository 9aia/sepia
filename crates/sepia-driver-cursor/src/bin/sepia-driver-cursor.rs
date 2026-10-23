//! `sepia-driver-cursor` — the Cursor `~/.cursor` driver binary.
//!
//! `--manifest` prints the manifest; with no flags it serves the
//! driver-wire protocol on stdio. The data dir comes from
//! `SEPIA_CURSOR_DIR` (default `~/.cursor`), matching the `apps/server`
//! env contract.

use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver::{
    AgentConfigStore, Capability, DRIVER_PROTOCOL, DriverManifest, StoreDriver, serve,
};
use sepia_driver_cursor::CursorStore;

fn cursor_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEPIA_CURSOR_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cursor")
}

/// `config.*` — the driver's config root shares the store's
/// `SEPIA_CURSOR_DIR` convention; a `dir` param naming
/// `<repo>/.cursor` hits project scope.
struct CursorConfig;

impl AgentConfigStore for CursorConfig {
    fn read_config(
        &self,
        dir: Option<PathBuf>,
    ) -> Result<sepia_core::agent_config::AgentConfig, sepia_core::StorageError> {
        sepia_driver_cursor::config::read(&dir.unwrap_or_else(cursor_dir))
    }

    fn write_config(
        &self,
        dir: Option<PathBuf>,
        config: &sepia_core::agent_config::AgentConfig,
    ) -> Result<Vec<sepia_core::agent_config::ConfigWriteAction>, sepia_core::StorageError> {
        sepia_driver_cursor::config::write(config, &dir.unwrap_or_else(cursor_dir))
    }
}

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "cursor".into(),
        label: "Cursor chats + transcripts".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: [Capability::SessionStore, Capability::AgentConfig]
            .into_iter()
            .collect(),
        // Cursor has no ACP runtime — sessions are read/history only.
        agent_command: None,
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "dir": { "type": "string", "description": ".cursor directory path (~/.cursor)" }
            }
        }),
        backend_type: Some("cursor".into()),
    }
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--manifest") {
        println!("{}", serde_json::to_string(&manifest()).unwrap_or_default());
        return Ok(());
    }
    let store = CursorStore::new(cursor_dir());
    let rt = tokio::runtime::Runtime::new()?;
    let driver = StoreDriver::new(manifest(), Arc::new(store)).with_config(Arc::new(CursorConfig));
    rt.block_on(serve(driver))
}
