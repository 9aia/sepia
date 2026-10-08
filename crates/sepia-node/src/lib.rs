//! sepia-node — the headless daemon. Composes:
//!
//! - `DriverRegistry::discover` → per-driver `RemoteStore`s → merged repo
//! - usable drivers → `AgentRuntime` specs for the control plane
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
use sepia_control::{
    AgentRuntime, ControlError, ControlPlane, ControlPlaneOptions, SessionRewinder,
};
use sepia_core::Session;
use sepia_core::rewind::RewindPlan;
use sepia_driver_host::DriverRegistry;
use sepia_driver_host::store::RemoteStore;
use sepia_driver_sdk::Capability;
use sepia_http::AppState;
use sepia_http::env::Env;
use sepia_http::routes::sessions::ImportTarget;
use sepia_meta::MetaStore;

/// Node-local state layout under `SEPIA_HOME` (default
/// `~/.local/share/sepia`).
#[derive(Clone, Debug)]
pub struct NodePaths {
    pub home: PathBuf,
    /// `<home>/meta.json` — the session overlay.
    pub meta_path: PathBuf,
    /// `<home>/node.json` — node identity (id + display name).
    pub node_path: PathBuf,
    /// `<home>/tokens.json` — issued pair credentials (sha256 hashes).
    pub tokens_path: PathBuf,
    /// `<home>/pair-code` — the one-time pairing code `sepia pair` writes.
    pub pair_code_path: PathBuf,
}

impl NodePaths {
    pub fn new(home: PathBuf) -> Self {
        Self {
            meta_path: home.join("meta.json"),
            node_path: home.join("node.json"),
            tokens_path: home.join("tokens.json"),
            pair_code_path: home.join("pair-code"),
            home,
        }
    }
}

/// Default `SEPIA_HOME`: `SEPIA_HOME` env or `~/.local/share/sepia`.
pub fn sepia_home() -> PathBuf {
    std::env::var_os("SEPIA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/sepia")))
        .unwrap_or_else(|| PathBuf::from(".sepia"))
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
                code: sepia_control::ControlErrorCode::Internal,
                message: format!("rewind failed: {e}"),
                cause: Some(e.to_string()),
            })
    }
}

/// What `build` wires together — the control plane plus everything the
/// HTTP layer mounts.
pub struct Node {
    pub plane: Arc<ControlPlane>,
    pub meta: MetaStore,
    pub registry: DriverRegistry,
    pub repo: Arc<dyn sepia_core::storage::SessionRepository>,
    pub paths: NodePaths,
}

/// Build the node: discover drivers, open the merged store, wire agents
/// and rewinders into the control plane.
///
/// Discovery warnings (bad manifests, unreachable drivers) are logged and
/// skipped — a missing driver is an absent agent, never a boot failure.
///
/// # Errors
/// Currently infallible composition-wise; returns `Err` if the meta
/// store's parent cannot be created.
pub async fn build(paths: &NodePaths, mut options: ControlPlaneOptions) -> std::io::Result<Node> {
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
                .is_some_and(|c| sepia_driver_host::command_resolves(c));
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

    let repo: Arc<dyn sepia_core::storage::SessionRepository> = Arc::new(merged);
    let plane = ControlPlane::new(Arc::clone(&repo), options);
    let meta = MetaStore::open(&paths.meta_path);

    Ok(Node {
        plane,
        meta,
        registry,
        repo,
        paths: paths.clone(),
    })
}

fn control_err(e: impl std::fmt::Display) -> ControlError {
    ControlError {
        code: sepia_control::ControlErrorCode::Internal,
        message: e.to_string(),
        cause: Some(e.to_string()),
    }
}

/// Build + serve the node — the whole `sepia-node` boot path, shared with
/// `sepia serve` so both binaries behave identically. Call on a tokio
/// runtime; returns when the listener fails or after graceful shutdown.
///
/// # Errors
/// `std::io::Error` on node build, bind, or serve failure.
pub async fn serve(env: &Env) -> std::io::Result<()> {
    let paths = NodePaths::new(env.home.clone());
    let node = build(
        &paths,
        ControlPlaneOptions {
            idle_ttl: Some(env.idle_ttl),
            sweep_interval: Some(env.sweep),
            probe_cwd: std::env::current_dir().ok(),
            file_history_dir: Some(env.claude_dir.join("file-history")),
            terminate_lock_holder: Some(Arc::new(|pid| {
                // SIGTERM — the documented takeover signal.
                #[cfg(unix)]
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                #[cfg(not(unix))]
                let _ = pid;
            })),
            ..ControlPlaneOptions::default()
        },
    )
    .await?;

    // Conversion seams — devin↔cline, like the TS `deps.convert`.
    let devin_store = node
        .registry
        .merged_store()
        .await
        .for_agent("devin")
        .map(|s| Arc::new(s.clone()) as Arc<dyn sepia_core::storage::SessionRepository>);
    let cline_dir = env.cline_dir.clone();
    let state = if let Some(devin) = &devin_store {
        let repo = Arc::clone(devin);
        let cline = cline_dir.clone();
        let convert = Arc::new(move |id: String, target: ImportTarget| {
            let repo = Arc::clone(&repo);
            let cline = cline.clone();
            Box::pin(async move {
                match target {
                    ImportTarget::Cline => {
                        sepia_convert::install_cline(&repo, &id, &cline, None, false)
                            .await
                            .map_err(|e| control_err(e.message))
                    }
                    ImportTarget::Devin => sepia_convert::import_cline(
                        &cline.join("sessions").join(&id),
                        None,
                        &repo,
                        false,
                    )
                    .await
                    .map_err(|e| control_err(e.message)),
                }
            }) as futures::future::BoxFuture<'static, Result<String, ControlError>>
        }) as sepia_http::ConvertSession;
        let repo2 = Arc::clone(devin);
        let import = Arc::new(move |session: Session, _target: ImportTarget| {
            let repo = Arc::clone(&repo2);
            Box::pin(async move {
                sepia_convert::import_session(&repo, &session)
                    .await
                    .map_err(|e| control_err(e.message))
            }) as futures::future::BoxFuture<'static, Result<String, ControlError>>
        }) as sepia_http::ImportSession;
        AppState::from_env(env, Arc::clone(&node.plane))
            .with_convert(convert)
            .with_import_session(import)
    } else {
        AppState::from_env(env, Arc::clone(&node.plane))
    };
    run(env, state, node).await
}

async fn run(env: &Env, state: AppState, node: Node) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind((env.host.as_str(), env.port)).await?;
    tracing::info!("sepia-node listening on {}:{}", env.host, env.port);
    let app = sepia_http::app(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("sepia-node shutting down");
            node.plane.close_all().await;
        })
        .await
}
