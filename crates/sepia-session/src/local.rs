//! The local session plane — everything `sepia` can do with the drivers
//! and agent CLIs on *this* machine, no daemon required. Composes:
//!
//! - `DriverRegistry::discover` → per-driver `RemoteStore`s → merged repo
//! - usable drivers → `AgentRuntime` specs for the session plane
//! - `Capability::Rewind` drivers → `SessionRewinder` adapters
//! - `MetaStore` overlay + `ControlPlane`
//!
//! Nothing here names a backend — agents appear when both their driver
//! binary and `agent_command` resolve.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sepia_acp::registry::builtin_agents;
use sepia_acp::{AcpConnection, AgentSpec, SpawnOptions, spawn_agent};
use sepia_core::Session;
use sepia_core::rewind::RewindPlan;
use sepia_core::storage::SessionRepository;
use sepia_driver::Capability;
use sepia_host::DriverRegistry;
use sepia_host::store::RemoteStore;
use sepia_meta::MetaStore;

use crate::{
    AgentRuntime, ControlError, ControlErrorCode, ControlPlane, ControlPlaneOptions,
    SessionRewinder,
};

/// Machine-local state layout under `SEPIA_HOME` (default
/// `~/.local/share/sepia`).
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    /// `<home>/meta.json` — the session overlay.
    pub meta_path: PathBuf,
    /// `<home>/node.json` — machine identity (id + display name),
    /// recorded as run-span provenance.
    pub node_path: PathBuf,
}

impl Paths {
    pub fn new(home: PathBuf) -> Self {
        Self {
            meta_path: home.join("meta.json"),
            node_path: std::env::var_os("SEPIA_NODE")
                .map_or_else(|| home.join("node.json"), PathBuf::from),
            home,
        }
    }
}

/// Machine identity persisted at `Paths::node_path` — `id` is minted
/// once and stable, `name` defaults to the hostname (`SEPIA_NAME`
/// overrides) and doubles as `RunSpan.node` provenance.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeIdentity {
    pub id: String,
    pub name: String,
}

/// Read the node identity, minting + persisting `node_<hex16>` when the
/// file is absent or unreadable.
pub fn node_identity(paths: &Paths) -> NodeIdentity {
    if let Ok(raw) = std::fs::read_to_string(&paths.node_path) {
        if let Ok(id) = serde_json::from_str::<NodeIdentity>(&raw) {
            return id;
        }
    }
    let identity = NodeIdentity {
        id: format!("node_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
        name: std::env::var("SEPIA_NAME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                nix::unistd::gethostname()
                    .ok()
                    .map(|h| h.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "node".into()),
    };
    if let Ok(json) = serde_json::to_vec(&identity) {
        if let Some(dir) = paths.node_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&paths.node_path, json);
    }
    identity
}

/// Default `SEPIA_HOME`: `SEPIA_HOME` env or `~/.local/share/sepia`.
pub fn sepia_home() -> PathBuf {
    std::env::var_os("SEPIA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/sepia")))
        .unwrap_or_else(|| PathBuf::from(".sepia"))
}

/// `ControlPlaneOptions` carrying the machine-local defaults a plain
/// `sepia` process uses — SIGTERM as the documented takeover signal, the
/// cwd as lock-probe root, Claude's default file-history dir. Callers
/// layer their own `idle_ttl`/`sweep_interval`/rewinders/agents on top.
#[must_use]
pub fn base_options() -> ControlPlaneOptions {
    ControlPlaneOptions {
        probe_cwd: std::env::current_dir().ok(),
        file_history_dir: sepia_home()
            .parent()
            .map(|p| p.join(".claude/file-history")),
        terminate_lock_holder: Some(Arc::new(|pid| {
            #[cfg(unix)]
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            #[cfg(not(unix))]
            let _ = pid;
        })),
        ..ControlPlaneOptions::default()
    }
}

/// An `AgentRuntime` backed by an `AgentSpec` — spawns the real ACP
/// subprocess; `SEPIA_AGENT_<ID>_COMMAND` overrides the argv.
struct SpecRuntime {
    spec: AgentSpec,
}

#[async_trait]
impl AgentRuntime for SpecRuntime {
    fn id(&self) -> &str {
        &self.spec.id
    }

    fn label(&self) -> &str {
        &self.spec.label
    }

    async fn spawn(
        &self,
        cwd: &str,
        model: Option<&str>,
        fallbacks: Option<&[String]>,
    ) -> Result<AcpConnection, sepia_acp::rpc::RpcError> {
        spawn_agent(
            &self.spec,
            &SpawnOptions {
                cwd: cwd.to_string(),
                env: None,
                model: model.map(str::to_string),
                fallbacks: fallbacks.map(<[String]>::to_vec),
            },
        )
        .await
    }
}

/// `session.rewind` over the driver wire — the driver picks its store
/// mechanism; the host translates RPC failures to `internal`.
struct DriverRewinder {
    store: RemoteStore,
}

#[async_trait]
impl SessionRewinder for DriverRewinder {
    async fn truncate(
        &self,
        session: &Session,
        plan: &RewindPlan,
        truncated: &Session,
    ) -> Result<(), ControlError> {
        self.store
            .rewind(session, plan, truncated)
            .await
            .map_err(|e| ControlError {
                code: ControlErrorCode::Internal,
                message: format!("rewind failed: {e}"),
                cause: Some(e.to_string()),
            })
    }
}

/// Caller overrides for [`Local::import_session`] — `id` re-keys the
/// copy (absent → a fresh id is minted); `cwd`/`title`/`model` patch the
/// IR before it lands in the target store.
#[derive(Clone, Debug, Default)]
pub struct ImportOverrides {
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
}

/// The fresh id an imported copy lands under. Cline ids double as the
/// session-dir name — `<epoch-ms>_<5 lowercase alnum>` (mirrors
/// `sepia_driver_cline::cline::cline_session_id`; the plane cannot link
/// a driver crate). Every other store takes a uuid, like v1 imports.
fn mint_import_id(agent: &str, created_at: f64) -> String {
    if agent == "cline" {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        let mut suffix = String::with_capacity(5);
        for byte in &uuid::Uuid::new_v4().into_bytes()[..5] {
            suffix.push(ALPHABET[usize::from(byte % ALPHABET.len() as u8)] as char);
        }
        format!("{}_{suffix}", (created_at * 1000.0) as i64)
    } else {
        uuid::Uuid::new_v4().to_string()
    }
}

/// What [`build`] wires together — the session plane plus everything a
/// service surface mounts.
pub struct Local {
    pub plane: Arc<ControlPlane>,
    pub meta: MetaStore,
    pub identity: NodeIdentity,
    pub registry: DriverRegistry,
    pub repo: Arc<dyn sepia_core::storage::SessionRepository>,
    pub paths: Paths,
}

/// Build the local plane: discover drivers, open the merged store, wire
/// agents and rewinders into the session plane.
///
/// Discovery warnings (bad manifests, unreachable drivers) are logged and
/// skipped — a missing driver is an absent agent, never a boot failure.
///
/// # Errors
/// Currently infallible composition-wise; returns `Err` if the meta
/// store's parent cannot be created.
pub async fn build(paths: &Paths, mut options: ControlPlaneOptions) -> std::io::Result<Local> {
    std::fs::create_dir_all(&paths.home)?;

    let (registry, warnings) = DriverRegistry::discover().await;
    for warning in warnings {
        tracing::warn!("driver discovery: {warning}");
    }

    // Agents: built-in specs whose driver exists AND whose command
    // resolves. `SEPIA_AGENT_<ID>_COMMAND` overrides the argv BEFORE the
    // resolution check — it substitutes the real agent binary (or a
    // test's mock).
    let agents: Vec<Arc<dyn AgentRuntime>> = builtin_agents()
        .into_iter()
        .map(|mut spec| {
            if let Some(override_cmd) =
                std::env::var(format!("SEPIA_AGENT_{}_COMMAND", spec.id.to_uppercase()))
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            {
                spec.command = override_cmd
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
            }
            spec
        })
        .filter(|spec| {
            let driver_present = registry
                .manifest(&spec.id)
                .is_some_and(|m| m.agent_command.is_some());
            let cmd_resolves = spec
                .command
                .first()
                .is_some_and(|c| sepia_host::command_resolves(c));
            driver_present && cmd_resolves
        })
        .map(|spec| Arc::new(SpecRuntime { spec }) as Arc<dyn AgentRuntime>)
        .collect();

    // Stores: every SessionStore driver, merged; extras degrade to misses.
    let merged = registry.merged_store().await;

    // Rewinders: drivers advertising Rewind get a SessionRewinder keyed by
    // the backend they own.
    let mut rewinders: HashMap<String, Arc<dyn SessionRewinder>> =
        std::mem::take(&mut options.rewinders);
    for entry in registry.with(&Capability::Rewind) {
        match entry.client().await {
            Ok(client) => {
                let store = RemoteStore::new(client, entry.manifest.id.clone());
                // Rewinders are looked up by agent id (see agent_for_backend),
                // which coincides with the driver id for the built-in set.
                rewinders.insert(
                    entry.manifest.id.clone(),
                    Arc::new(DriverRewinder { store }),
                );
            }
            Err(e) => {
                tracing::warn!("driver {} unwinder unavailable: {e}", entry.manifest.id);
            }
        }
    }
    options.rewinders = rewinders;
    options.agents = agents;

    let meta = MetaStore::open(&paths.meta_path);
    if options.meta.is_none() {
        options.meta = Some(meta.clone());
    }
    let identity = node_identity(paths);
    if options.node_name.is_none() {
        options.node_name = Some(identity.name.clone());
    }

    let repo: Arc<dyn sepia_core::storage::SessionRepository> = Arc::new(merged);
    let plane = ControlPlane::new(Arc::clone(&repo), options);

    Ok(Local {
        plane,
        meta,
        identity,
        registry,
        repo,
        paths: paths.clone(),
    })
}

impl Local {
    /// The `session.save` endpoint for `agent` — `unknown_agent` when no
    /// driver claims the id or it lacks `SessionWrite`.
    async fn write_store(&self, agent: &str) -> Result<RemoteStore, ControlError> {
        let Some(entry) = self.registry.entry(agent) else {
            return Err(ControlError::new(
                ControlErrorCode::UnknownAgent,
                format!("unknown agent: {agent}"),
            ));
        };
        if !entry
            .manifest
            .capabilities
            .contains(&Capability::SessionWrite)
        {
            return Err(ControlError::new(
                ControlErrorCode::UnknownAgent,
                format!("agent {agent} has no write-capable session store"),
            ));
        }
        let client = entry.client().await.map_err(|e| {
            ControlError::caused(
                ControlErrorCode::Internal,
                format!("driver {agent} unavailable"),
                e,
            )
        })?;
        Ok(RemoteStore::new(client, agent.to_string()))
    }

    /// Write `session` into `agent`'s store via `session.save` over the
    /// driver wire — the drivers' save is the install path (cline writes
    /// manifest + transcript + index row, claude its canonical layout).
    /// Imports are copies, not moves: a fresh id is minted per the
    /// target's convention unless `overrides.id` re-keys it.
    ///
    /// # Errors
    /// `unknown_agent` without a SessionWrite driver for `agent`;
    /// `internal` on driver spawn or store-write failure.
    pub async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError> {
        let store = self.write_store(agent).await?;
        let mut copy = session.clone();
        copy.id = overrides
            .id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| mint_import_id(agent, session.created_at));
        if let Some(cwd) = &overrides.cwd {
            copy.working_directory = cwd.clone();
        }
        if let Some(title) = &overrides.title {
            copy.title = title.clone();
        }
        if let Some(model) = &overrides.model {
            copy.model = model.clone();
        }
        store.save(&copy).await.map_err(|e| {
            ControlError::caused(
                ControlErrorCode::Internal,
                format!("import into {agent} failed"),
                e.message,
            )
        })?;
        Ok(copy.id)
    }

    /// The node-local shortcut over [`Local::import_session`]: read `id`
    /// from the merged store (`from` disambiguates cross-store id
    /// collisions) and copy it into `agent`'s store — saves the
    /// orchestrator an export/import IR round-trip.
    ///
    /// # Errors
    /// `not_found` on unknown `id`; the [`Local::import_session`] errors
    /// otherwise.
    pub async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError> {
        let session = self.plane.get_session(id, from).await?;
        self.import_session(agent, &session, &ImportOverrides::default())
            .await
    }
}
