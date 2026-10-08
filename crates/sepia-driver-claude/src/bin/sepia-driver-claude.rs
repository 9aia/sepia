//! `sepia-driver-claude` — the Claude Code `projects/*.jsonl` driver binary.
//!
//! `--manifest` prints the manifest; with no flags it serves the
//! driver-wire protocol on stdio. The store root comes from
//! `SEPIA_CLAUDE_DIR` (default `~/.claude`); transcripts live under its
//! `projects/` directory.

use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver_claude::ClaudeStore;
use sepia_driver_sdk::{Capability, DRIVER_PROTOCOL, DriverManifest, StoreDriver, serve};

fn claude_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEPIA_CLAUDE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
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
    let driver = StoreDriver::new(manifest(), store.clone()).with_truncator(store);
    rt.block_on(serve(driver))
}
