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
    client: tokio::sync::Mutex<Option<Result<Arc<DriverClient>, String>>>,
}

impl DriverEntry {
    /// The spawned driver client, respawning on demand when the last
    /// process died (stdout closed / crashed). Held under a lock so
    /// concurrent callers share one spawn.
    ///
    /// # Errors
    /// The spawn error string from the most recent attempt.
    pub async fn client(&self) -> Result<Arc<DriverClient>, String> {
        let mut slot = self.client.lock().await;
        // Respawn when the cached client died or never spawned.
        let stale = match slot.as_ref() {
            Some(Ok(c)) => c.is_closed(),
            Some(Err(_)) | None => true,
        };
        if stale {
            *slot = Some(
                DriverClient::spawn(&self.binary, &[])
                    .await
                    .map(Arc::new)
                    .map_err(|e| format!("spawn {}: {e}", self.binary.display())),
            );
        }
        match slot.as_ref() {
            Some(Ok(client)) => Ok(Arc::clone(client)),
            Some(Err(e)) => Err(e.clone()),
            None => Err("driver spawn produced no client".to_string()),
        }
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
                            client: tokio::sync::Mutex::new(None),
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
                client: tokio::sync::Mutex::new(None),
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
                    client,
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
pub fn command_resolves(command: &str) -> bool {
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
