//! `sepia-driver-claude` — the Claude Code `projects/*.jsonl` driver binary.
//!
//! `--manifest` prints the manifest; with no flags it serves the
//! driver-wire protocol on stdio. The store root comes from
//! `SEPIA_CLAUDE_DIR` (default `~/.claude`); transcripts live under its
//! `projects/` directory. `config.*` calls read/write a `.claude` dir —
//! `dir` param, else `SEPIA_CLAUDE_DIR` → `~/.claude` (a `<repo>/.claude`
//! path reads project scope).

use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver::{
    AgentConfigStore, Capability, DRIVER_PROTOCOL, DriverManifest, StoreDriver, serve,
};
use sepia_driver_claude::ClaudeStore;

fn claude_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEPIA_CLAUDE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
}

/// `config.*` — the driver's config root shares the store's
/// `SEPIA_CLAUDE_DIR` convention; a `dir` param naming
/// `<repo>/.claude` hits project scope.
struct ClaudeConfig;

impl AgentConfigStore for ClaudeConfig {
    fn read_config(
        &self,
        dir: Option<PathBuf>,
    ) -> Result<sepia_core::agent_config::AgentConfig, sepia_core::StorageError> {
        sepia_driver_claude::config::read(&dir.unwrap_or_else(claude_dir))
    }

    fn write_config(
        &self,
        dir: Option<PathBuf>,
        config: &sepia_core::agent_config::AgentConfig,
    ) -> Result<Vec<sepia_core::agent_config::ConfigWriteAction>, sepia_core::StorageError> {
        sepia_driver_claude::config::write(config, &dir.unwrap_or_else(claude_dir))
    }
}

fn manifest() -> DriverManifest {
    DriverManifest {
        id: "claude".into(),
        label: "Claude Code projects".into(),
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
        // Claude Code has no native ACP runtime — `claude-agent-acp`
        // (`@agentclientprotocol/claude-agent-acp`) bridges it.
        agent_command: Some("claude-agent-acp".into()),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "dir": { "type": "string", "description": ".claude directory path" }
            }
        }),
        backend_type: Some("claude".into()),
    }
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--manifest") {
        println!("{}", serde_json::to_string(&manifest()).unwrap_or_default());
        return Ok(());
    }
    let store = Arc::new(ClaudeStore::for_claude_dir(&claude_dir()));
    let rt = tokio::runtime::Runtime::new()?;
    let driver = StoreDriver::new(manifest(), store.clone())
        .with_truncator(store)
        .with_config(Arc::new(ClaudeConfig));
    rt.block_on(serve(driver))
}
