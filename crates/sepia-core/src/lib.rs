//! sepia-core — the pure domain half of the system: session IR, the
//! `SessionRepository` port, agent-config IR, restore/rewind planners and
//! the frontmatter subset parser. **Zero I/O dependencies** — no tokio, no
//! fs; must compile to `wasm32-unknown-unknown` unchanged so one IR serves
//! node, hub and browser.

pub mod agent_config;
pub mod domain;
pub mod frontmatter;
pub mod restore;
pub mod rewind;
pub mod shared;
pub mod storage;
pub mod wire;

pub use agent_config::*;
pub use domain::*;
pub use restore::{FILE_HISTORY_KIND, FileRestorePlan};
pub use shared::{
    SESSION_CHECKPOINTS_KEY, apply_tool_call_outcomes, checkpoints_from_metadata,
    decode_project_dir, default_cogs_json, default_session_metadata, encode_project_dir,
    tool_node_outcomes,
};
pub use storage::{
    NodesWindowOptions, REQUIRED_TABLES, SessionNodeWindow, SessionRepository, needs_migration,
};
