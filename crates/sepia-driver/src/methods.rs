//! Driver-wire protocol v1 method names and their param/result shapes.

use serde::{Deserialize, Serialize};

pub const DRIVER_MANIFEST: &str = "driver.manifest";
pub const SESSION_LIST: &str = "session.list";
pub const SESSION_GET: &str = "session.get";
pub const SESSION_SUMMARY: &str = "session.summary";
pub const SESSION_HISTORY: &str = "session.history";
pub const SESSION_SAVE: &str = "session.save";
pub const SESSION_RENAME: &str = "session.rename";
pub const SESSION_DELETE: &str = "session.delete";
pub const SESSION_CHECKPOINTS: &str = "session.checkpoints";
pub const SESSION_REWIND: &str = "session.rewind";
pub const FILE_RESTORE: &str = "file.restore";
pub const CONFIG_READ: &str = "config.read";
pub const CONFIG_WRITE: &str = "config.write";

/// Every method name v1 defines — the host validates requests against it.
pub const ALL: [&str; 13] = [
    DRIVER_MANIFEST,
    SESSION_LIST,
    SESSION_GET,
    SESSION_SUMMARY,
    SESSION_HISTORY,
    SESSION_SAVE,
    SESSION_RENAME,
    SESSION_DELETE,
    SESSION_CHECKPOINTS,
    SESSION_REWIND,
    FILE_RESTORE,
    CONFIG_READ,
    CONFIG_WRITE,
];

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdParams {
    pub id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// `session.rewind` — the control plane ships the full pre-rewind IR, the
/// computed cut, and the already-truncated IR; the driver picks its
/// store mechanism.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindParams {
    pub session: sepia_core::Session,
    pub plan: sepia_core::rewind::RewindPlan,
    pub truncated: sepia_core::Session,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryParams {
    pub id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreParams {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// Required confirmation gate — mirrors the destructive-op rule.
    pub confirm: bool,
}

/// `config.read` — `dir` is the config root the driver's adapter
/// expects (a `.claude`/`.cursor`/`devin` dir, or a Cline workspace
/// root); absent, the driver resolves its own default (env override or
/// user dir / process cwd).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReadParams {
    #[serde(default)]
    pub dir: Option<std::path::PathBuf>,
}

/// `config.write` — `config` is a config IR JSON payload (the
/// `config export` shape or a full serialized `AgentConfig`; the driver
/// decodes it with `config_from_json`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigWriteParams {
    #[serde(default)]
    pub dir: Option<std::path::PathBuf>,
    pub config: serde_json::Value,
}

/// `config.write` result — the per-file write report.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigWriteResult {
    pub actions: Vec<sepia_core::agent_config::ConfigWriteAction>,
}

/// Per-file outcome of a restore execution.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: Vec<String>,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}
