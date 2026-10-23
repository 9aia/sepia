//! `sepia-driver-cline` — the Cline `~/.cline/data` driver binary.
//!
//! `--manifest` prints the manifest; with no flags it serves the
//! driver-wire protocol on stdio. The data dir comes from
//! `SEPIA_CLINE_DIR` (default `~/.cline/data`); `config.*` calls default
//! to the process cwd — Cline config is workspace-rooted.

use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver::{
    AgentConfigStore, Capability, DRIVER_PROTOCOL, DriverManifest, StoreDriver, serve,
};
use sepia_driver_cline::ClineStore;

fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEPIA_CLINE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cline/data")
}

/// `config.*` — Cline's portable config lives in the workspace root
/// (`.clinerules`, `cline_mcp_settings.json`), so an absent `dir`
/// resolves to the driver's cwd — the same default `sepia config`'s
/// `--cline-dir` flag uses.
struct ClineConfig;

impl AgentConfigStore for ClineConfig {
    fn read_config(
        &self,
        dir: Option<PathBuf>,
    ) -> Result<sepia_core::agent_config::AgentConfig, sepia_core::StorageError> {
        let dir = dir
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        sepia_driver_cline::config::read(&dir)
    }

    fn write_config(
        &self,
        dir: Option<PathBuf>,
        config: &sepia_core::agent_config::AgentConfig,
    ) -> Result<Vec<sepia_core::agent_config::ConfigWriteAction>, sepia_core::StorageError> {
        let dir = dir
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        sepia_driver_cline::config::write(config, &dir)
    }
}

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "cline".into(),
        label: "Cline session dirs".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: DRIVER_PROTOCOL,
        capabilities: [
            Capability::SessionStore,
            Capability::SessionWrite,
            Capability::Checkpoints,
            Capability::Rewind,
            Capability::AgentConfig,
        ]
        .into_iter()
        .collect(),
        agent_command: Some("cline --acp".into()),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "dataDir": { "type": "string", "description": "Cline data dir (~/.cline/data)" }
            }
        }),
        backend_type: Some("cline".into()),
    }
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--manifest") {
        println!("{}", serde_json::to_string(&manifest()).unwrap_or_default());
        return Ok(());
    }
    let store = Arc::new(ClineStore::new(data_dir()));
    let driver = StoreDriver::new(manifest(), store.clone())
        .with_truncator(store)
        .with_config(Arc::new(ClineConfig));
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve(driver))
}
