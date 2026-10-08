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

/// Every method name v1 defines — the host validates requests against it.
pub const ALL: [&str; 11] = [
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
];

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdParams {
    pub id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
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
pub struct RewindParams {
    pub id: String,
    /// Exactly one of nodeId/turns/checkpoint must be present.
    #[serde(default)]
    pub node_id: Option<i64>,
    #[serde(default)]
    pub turns: Option<i64>,
    #[serde(default)]
    pub checkpoint: Option<String>,
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
