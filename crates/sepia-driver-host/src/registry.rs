//! `DriverRegistry` — manifests + live driver clients indexed by
//! capability. Consumers ask by capability, never by driver name.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use sepia_driver_sdk::manifest::{Capability, DriverManifest};
use sepia_driver_sdk::methods;
use serde_json::json;

use crate::client::DriverClient;
use crate::discover;

/// A discovered driver — its manifest and (lazily spawned) client.
pub struct DriverEntry {
    pub binary: PathBuf,
    pub manifest: DriverManifest,
    client: tokio::sync::OnceCell<Result<Arc<DriverClient>, String>>,
}

impl DriverEntry {
    /// The live client, spawned on first use.
    ///
    /// # Errors
    /// Carries the spawn failure string on repeated access too.
    pub async fn client(&self) -> Result<&Arc<DriverClient>, String> {
        self.client
            .get_or_init(|| async {
                DriverClient::spawn(&self.binary, &[])
                    .await
                    .map(Arc::new)
                    .map_err(|e| format!("spawn {}: {e}", self.binary.display()))
            })
            .await
            .as_ref()
            .map_err(Clone::clone)
    }
}

#[derive(Default)]
pub struct DriverRegistry {
    entries: BTreeMap<String, DriverEntry>,
}

impl DriverRegistry {
    /// Discover binaries + probe manifests without spawning any driver.
    /// Bad drivers are logged and skipped, never fatal.
    pub async fn discover() -> (Self, Vec<String>) {
        let mut registry = Self::default();
        let mut problems = Vec::new();
        for binary in discover::find_driver_binaries() {
            match discover::probe_manifest(&binary).await {
                Ok(manifest) => {
                    let id = manifest.id.clone();
                    registry.entries.insert(
                        id,
                        DriverEntry {
                            binary,
                            manifest,
                            client: tokio::sync::OnceCell::new(),
                        },
                    );
                }
                Err(problem) => problems.push(problem),
            }
        }
        (registry, problems)
    }

    /// Insert a pre-probed entry (tests, explicit paths).
    pub fn insert(&mut self, binary: PathBuf, manifest: DriverManifest) {
        self.entries.insert(
            manifest.id.clone(),
            DriverEntry {
                binary,
                manifest,
                client: tokio::sync::OnceCell::new(),
            },
        );
    }

    pub fn manifests(&self) -> impl Iterator<Item = &DriverManifest> {
        self.entries.values().map(|e| &e.manifest)
    }

    pub fn manifest(&self, id: &str) -> Option<&DriverManifest> {
        self.entries.get(id).map(|e| &e.manifest)
    }

    pub fn entry(&self, id: &str) -> Option<&DriverEntry> {
        self.entries.get(id)
    }

    /// Drivers advertising `capability`.
    pub fn with(&self, capability: &Capability) -> Vec<&DriverEntry> {
        self.entries
            .values()
            .filter(|e| e.manifest.capabilities.contains(capability))
            .collect()
    }

    /// Agents fully usable: driver present AND its `agent_command`
    /// resolves on PATH.
    pub fn usable_agents(&self) -> Vec<&DriverManifest> {
        self.entries
            .values()
            .map(|e| &e.manifest)
            .filter(|m| {
                m.agent_command
                    .as_ref()
                    .is_none_or(|cmd| command_resolves(cmd))
            })
            .collect()
    }

    /// A merged `SessionRepository` across every SessionStore driver —
    /// ids narrow via `agent_id` where they collide.
    pub async fn merged_store(&self) -> crate::store::MergedStore {
        let mut stores = Vec::new();
        for entry in self.with(&Capability::SessionStore) {
            match entry.client().await {
                Ok(client) => stores.push(crate::store::RemoteStore::new(
                    Arc::clone(client),
                    entry.manifest.id.clone(),
                )),
                Err(e) => {
                    eprintln!("sepia: driver {} unavailable: {e}", entry.manifest.id);
                }
            }
        }
        crate::store::MergedStore::new(stores)
    }
}

/// Whether `command` resolves — first word on PATH.
fn command_resolves(command: &str) -> bool {
    let Some(program) = command.split_whitespace().next() else {
        return false;
    };
    if program.contains('/') {
        return PathBuf::from(program).exists();
    }
    std::env::var("PATH")
        .is_ok_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).exists()))
}

/// Fetch a single driver's manifest over RPC (used to refresh after spawn).
pub async fn refresh_manifest(client: &DriverClient) -> Result<DriverManifest, String> {
    let value = client
        .call(methods::DRIVER_MANIFEST, json!({}))
        .await
        .map_err(|e| e.message)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
