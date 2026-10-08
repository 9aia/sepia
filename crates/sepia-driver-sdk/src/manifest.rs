//! Driver manifest — the static self-description every driver emits via
//! `sepia-driver-<name> --manifest` and mirrors over `driver.manifest`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Driver-wire protocol version implemented by this crate.
pub const DRIVER_PROTOCOL: u32 = 1;

/// What a driver can do — consumers ask the registry by capability, never
/// by driver name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    /// list/get/summary/history reads over a session store.
    SessionStore,
    /// Write side of the store — `session.save`/`session.rename`/`session.delete`.
    SessionWrite,
    /// Workspace snapshot refs (`session.checkpoints`).
    Checkpoints,
    /// File restore from recorded diffs/checkpoints.
    Restore,
    /// Transcript truncation (`session.rewind`).
    Rewind,
    /// Cross-store conversion between two backends.
    Convert { from: String, to: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverManifest {
    /// Stable id — `"devin"`, `"cline"`, …
    pub id: String,
    pub label: String,
    pub version: String,
    /// Driver-wire protocol version; hosts refuse drivers whose major
    /// differs from [`DRIVER_PROTOCOL`].
    pub protocol: u32,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    /// The agent runtime command this driver's sessions resume under
    /// (e.g. `"cline --acp"`) — the control plane advertises the agent only
    /// when the driver AND this command both resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
    /// JSON Schema describing the driver's configuration object.
    #[serde(default)]
    pub config_schema: Value,
    /// Store backend label stamped onto sessions (`backendType`).
    #[serde(default)]
    pub backend_type: Option<String>,
}

impl DriverManifest {
    /// Reject manifests a host cannot safely drive.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("manifest id is empty".into());
        }
        if self.protocol != DRIVER_PROTOCOL {
            return Err(format!(
                "driver protocol {} != host protocol {DRIVER_PROTOCOL}",
                self.protocol
            ));
        }
        Ok(())
    }
}
