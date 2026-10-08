//! `ControlPlane` — live-session ownership, lock probing, attach/takeover,
//! prompt/cancel/permission, restore/rewind. Port of
//! `packages/session-control/src/ControlPlane.ts` — the state machine is
//! identical; Effect's fibers and finalizers map to tokio tasks, mutexes,
//! and drop.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use sepia_acp::{AcpCapabilities, AcpConnection, AcpSessionInfo, PromptPart, rpc::RpcError};
use sepia_core::restore::{self, FileRestorePlan};
use sepia_core::rewind::{self, RewindTarget};
use sepia_core::storage::{NodesWindowOptions, SessionRepository};
use sepia_core::{Role, Session, StorageError, ToolCall};
use sepia_proto::SessionEvent;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinSet;

use crate::merged::agent_for_backend;
use crate::translate::Translator;
use crate::types::{
    AgentInfo, AgentRuntime, AttachResult, ControlError, ControlErrorCode, ControlPlaneOptions,
    HistoryMessage, HistoryOptions, HistoryPage, RestoreAction, RestoreExec, RestoreRequest,
    RestoreResult, RestoredFile, RewindRequest, RewindResult, SessionRewinder, SessionSummary,
    SkippedFile,
};

const DEFAULT_HISTORY_LIMIT: usize = 500;
const DEFAULT_LOCK_TTL: Duration = Duration::from_secs(5);
const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(600);
const DEFAULT_SWEEP: Duration = Duration::from_secs(30);
// A signaled holder needs a moment to flush and drop the session lock —
// poll session/list for this long before giving the load a shot anyway.
const TAKEOVER_SETTLE: Duration = Duration::from_millis(800);
const TAKEOVER_POLL: Duration = Duration::from_millis(100);
// A store lock can lag the holder's exit — an explicit takeover retries
// the load once after this delay.
const TAKEOVER_RETRY_DELAY: Duration = Duration::from_millis(300);
// A pooled probe conn unused for this many lock TTLs is evicted by the
// next probe — floored so a 0 lock TTL doesn't churn agent spawns.
const PROBE_IDLE_FACTOR: u32 = 10;
const MIN_PROBE_IDLE: Duration = Duration::from_secs(60);
const EVENT_BUFFER: usize = 512;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

fn env_duration(key: &str, fallback: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(fallback, Duration::from_millis)
}

fn env_number(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(fallback)
}

fn control_error(code: ControlErrorCode, message: impl Into<String>) -> ControlError {
    ControlError::new(code, message)
}

fn acp_error(message: &'static str) -> impl Fn(RpcError) -> ControlError {
    move |e| ControlError::caused(ControlErrorCode::Internal, message, format!("{e:?}"))
}

fn storage_error(message: &'static str) -> impl Fn(StorageError) -> ControlError {
    move |e| ControlError::caused(ControlErrorCode::Internal, message, e.message)
}

/// A file tracked by a file-history snapshot — see `restore_file_history`.
struct CoveredFile {
    tracked: String,
    abs: PathBuf,
    backup: Option<String>,
}

/// One live (attached) session: the ACP connection plus the event fan-out
/// its listeners subscribe to.
struct LiveSession {
    conn: Arc<AcpConnection>,
    events: broadcast::Sender<Vec<SessionEvent>>,
    /// update/permission → event translation tasks.
    forward_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// The event translator — shared between forward tasks and the ops
    /// that emit run boundaries; std mutex, held only inside translate.
    translator: Arc<StdMutex<Translator>>,
    cwd: String,
    title: String,
    agent_id: String,
    busy: bool,
    /// Set while the session has no listeners and no in-flight turn.
    idle_since: Option<Instant>,
    /// When the session went live — pins `updated_at` for unflushed
    /// sessions.
    attached_at_ms: f64,
}

/// One `session/list` fan-out across every registered agent. `merged` is
/// the union keyed by session id — per-agent views can disagree on a
/// colliding id, so `locked: true` in ANY agent's view counts as held.
/// `by_agent` keeps each agent's own view so a caller can prefer the
/// report from the agent that owns the session's backend.
#[derive(Clone, Default)]
struct LockProbe {
    merged: HashMap<String, AcpSessionInfo>,
    by_agent: HashMap<String, HashMap<String, AcpSessionInfo>>,
}

impl LockProbe {
    /// The owning agent's view of a session, falling back to the merged
    /// union when that agent did not list the id at all.
    fn info_for(&self, agent_id: &str, session_id: &str) -> Option<&AcpSessionInfo> {
        self.by_agent
            .get(agent_id)
            .and_then(|v| v.get(session_id))
            .or_else(|| self.merged.get(session_id))
    }
}

/// What one probe round shares between the per-agent tasks.
struct ProbeContext {
    cwd: PathBuf,
    idle: Duration,
    at: Instant,
}

/// A pooled, internal `session/list` connection for one agent. Kept out
/// of the live map — no translator, no listeners — and retired by
/// `close_all`. `at` is the last successful probe verify.
struct ProbeConn {
    conn: Arc<AcpConnection>,
    at: Instant,
}

type AttachFuture = Shared<BoxFuture<'static, Result<AttachResult, ControlError>>>;

#[derive(Default)]
struct State {
    live: HashMap<String, LiveSession>,
    probe_conns: HashMap<String, ProbeConn>,
    /// (token, shared) — the token guards against evicting a newer
    /// pending attach when an older one finishes.
    pending_attaches: HashMap<String, (u64, AttachFuture)>,
    lock_cache: Option<(Instant, LockProbe)>,
    probed_capabilities: HashMap<String, AcpCapabilities>,
}

/// The sepia control plane — construct with [`ControlPlane::new`], which
/// hands back an `Arc` the methods and the idle sweep share.
pub struct ControlPlane {
    repo: Arc<dyn SessionRepository>,
    agents: Vec<Arc<dyn AgentRuntime>>,
    default_agent_id: Option<String>,
    probe_cwd: PathBuf,
    idle_ttl: Option<Duration>,
    sweep_interval: Duration,
    probe_idle: Option<Duration>,
    terminate_lock_holder: Arc<dyn Fn(i64) + Send + Sync>,
    restore_exec: Arc<dyn RestoreExec>,
    file_history_dir: PathBuf,
    rewinders: HashMap<String, Arc<dyn SessionRewinder>>,
    state: Mutex<State>,
    me: Weak<Self>,
    stop_sweep: AtomicBool,
    attach_seq: std::sync::atomic::AtomicU64,
}

impl ControlPlane {
    /// Build the control plane; starts the idle sweep when a TTL applies.
    pub fn new(repo: Arc<dyn SessionRepository>, options: ControlPlaneOptions) -> Arc<Self> {
        let idle_ttl = options
            .idle_ttl
            .unwrap_or_else(|| env_duration("SEPIA_IDLE_TTL_MS", DEFAULT_IDLE_TTL));
        let sweep_interval = options
            .sweep_interval
            .unwrap_or_else(|| env_duration("SEPIA_SWEEP_MS", DEFAULT_SWEEP));
        let plane = Arc::new_cyclic(|me| Self {
            repo,
            agents: options.agents,
            default_agent_id: options.default_agent_id,
            probe_cwd: options
                .probe_cwd
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from(".")),
            idle_ttl: (idle_ttl > Duration::ZERO).then_some(idle_ttl),
            sweep_interval,
            probe_idle: options.probe_idle,
            terminate_lock_holder: options
                .terminate_lock_holder
                .unwrap_or_else(|| Arc::new(default_terminate)),
            restore_exec: options
                .restore_exec
                .unwrap_or_else(|| Arc::new(crate::exec::DefaultRestoreExec)),
            file_history_dir: options.file_history_dir.unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("/"), PathBuf::from)
                    .join(".claude/file-history")
            }),
            rewinders: options.rewinders,
            state: Mutex::new(State::default()),
            me: me.clone(),
            stop_sweep: AtomicBool::new(false),
            attach_seq: std::sync::atomic::AtomicU64::new(0),
        });
        if plane.idle_ttl.is_some() {
            let weak = Arc::downgrade(&plane);
            let sweep = plane.sweep_interval.max(Duration::from_millis(1));
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(sweep);
                loop {
                    interval.tick().await;
                    let Some(plane) = weak.upgrade() else {
                        return;
                    };
                    if plane.stop_sweep.load(Ordering::SeqCst) {
                        return;
                    }
                    plane.sweep_idle().await;
                }
            });
        }
        plane
    }

    /// A `&self` method needs `Arc<Self>` to spawn 'static work; `me`
    /// provides it (constructed via `Arc::new_cyclic` — always upgradeable
    /// while the plane lives).
    fn shared(&self) -> Arc<Self> {
        self.me
            .upgrade()
            .unwrap_or_else(|| unreachable!("ControlPlane outlives its own methods"))
    }

    fn emit(live: &LiveSession, events: Vec<SessionEvent>) {
        if !events.is_empty() {
            let _ = live.events.send(events);
        }
    }

    /// `idle_since` tracks "no listeners and no in-flight turn".
    fn touch_idle(live: &mut LiveSession) {
        live.idle_since = if live.busy || live.events.receiver_count() > 0 {
            None
        } else {
            Some(Instant::now())
        };
    }

    /// Can this agent answer a lock probe at all? `session_list` is the
    /// RPC the probe calls; `load_session` marks an agent that can hold
    /// the kind of lock a takeover needs to release. `None` = never
    /// spawned, so probe once and let the spawn learn them.
    fn probeable(capabilities: Option<&AcpCapabilities>) -> bool {
        capabilities.is_none_or(|c| c.session_list && c.load_session)
    }

    async fn spawn_agent(
        &self,
        agent: &Arc<dyn AgentRuntime>,
        message: &'static str,
        cwd: &str,
        model: Option<&str>,
        fallbacks: Option<&[String]>,
    ) -> Result<Arc<AcpConnection>, ControlError> {
        let conn = agent
            .spawn(cwd, model, fallbacks)
            .await
            .map(Arc::new)
            .map_err(acp_error(message))?;
        let capabilities = conn.capabilities().await;
        self.state
            .lock()
            .await
            .probed_capabilities
            .insert(agent.id().to_string(), capabilities);
        Ok(conn)
    }

    async fn close_agent(conn: Arc<AcpConnection>) {
        conn.close().await;
    }

    /// Live sessions are keyed by bare id; when the caller scopes to an
    /// agent, a live entry for a different agent's colliding id must not
    /// match.
    fn live_for<'a>(state: &'a State, id: &str, agent_id: Option<&str>) -> Option<&'a LiveSession> {
        state
            .live
            .get(id)
            .filter(|live| agent_id.is_none_or(|a| live.agent_id == a))
    }

    async fn list_on(conn: &AcpConnection) -> Vec<AcpSessionInfo> {
        conn.list_sessions().await.unwrap_or_default()
    }

    /// One `session/list` per registered agent — a lock held under a
    /// backend the default agent can't see still surfaces. A live
    /// attach's connection is reused for its own agent; `borrowed`
    /// covers a connection about to go live. Every other agent is served
    /// by a pooled probe connection: spawned on first use, verified by
    /// the probe RPC itself, replaced by one fresh spawn on failure. An
    /// agent already probed as incapable is skipped. Probes run in
    /// parallel and a failing probe contributes an empty view.
    async fn probe_locks(
        &self,
        cwd: &Path,
        lock_ttl: Duration,
        borrowed: Option<(&str, Arc<AcpConnection>)>,
    ) -> LockProbe {
        let now = Instant::now();
        let probe_idle = self
            .probe_idle
            .unwrap_or_else(|| (lock_ttl * PROBE_IDLE_FACTOR).max(MIN_PROBE_IDLE));
        let plane = self.shared();

        // Reusable conns (live + borrowed) and pool/capability snapshots
        // under one lock — the fan-out tasks own clones.
        let (reusable, pools, probed) = {
            let state = self.state.lock().await;
            let mut map: HashMap<String, Arc<AcpConnection>> = HashMap::new();
            for live in state.live.values() {
                map.entry(live.agent_id.clone())
                    .or_insert_with(|| Arc::clone(&live.conn));
            }
            if let Some((agent_id, conn)) = &borrowed {
                map.insert((*agent_id).to_string(), Arc::clone(conn));
            }
            let pools: HashMap<String, (Arc<AcpConnection>, Instant)> = state
                .probe_conns
                .iter()
                .map(|(id, p)| (id.clone(), (Arc::clone(&p.conn), p.at)))
                .collect();
            let probed = state.probed_capabilities.clone();
            (map, pools, probed)
        };

        let mut set = JoinSet::new();
        for agent in &self.agents {
            let plane = Arc::clone(&plane);
            let agent = Arc::clone(agent);
            let reusable = reusable.get(agent.id()).cloned();
            let pool = pools.get(agent.id()).cloned();
            let probed = probed.get(agent.id()).cloned();
            let probe_ctx = ProbeContext {
                cwd: cwd.to_path_buf(),
                idle: probe_idle,
                at: now,
            };
            set.spawn(async move {
                (
                    agent.id().to_string(),
                    plane
                        .probe_one(&agent, reusable, pool, probed, &probe_ctx)
                        .await,
                )
            });
        }

        let mut probe = LockProbe::default();
        while let Some(result) = set.join_next().await {
            let Ok((agent_id, infos)) = result else {
                continue;
            };
            let view = probe.by_agent.entry(agent_id).or_default();
            for info in infos {
                view.insert(info.session_id.clone(), info.clone());
                let current = probe.merged.get(&info.session_id);
                // A session locked in any agent's view counts as held.
                if current.is_none() || (info.locked && !current.is_some_and(|c| c.locked)) {
                    probe.merged.insert(info.session_id.clone(), info);
                }
            }
        }
        probe
    }

    /// One agent's contribution to the lock probe — see [`probe_locks`].
    async fn probe_one(
        self: &Arc<Self>,
        agent: &Arc<dyn AgentRuntime>,
        reusable: Option<Arc<AcpConnection>>,
        pool: Option<(Arc<AcpConnection>, Instant)>,
        probed: Option<AcpCapabilities>,
        probe_ctx: &ProbeContext,
    ) -> Vec<AcpSessionInfo> {
        let agent_id = agent.id().to_string();
        if let Some(conn) = reusable {
            if !Self::probeable(Some(&conn.capabilities().await)) {
                return Vec::new();
            }
            return Self::list_on(&conn).await;
        }
        if let Some((conn, at)) = pool {
            if probe_ctx.at.duration_since(at) >= probe_ctx.idle {
                // Stale — retire it on sight rather than keep a subprocess
                // alive on a node that isn't probing anymore.
                if let Some(pooled) = self.state.lock().await.probe_conns.remove(&agent_id) {
                    Self::close_agent(pooled.conn).await;
                }
            } else if !Self::probeable(Some(&conn.capabilities().await)) {
                // The capability verdict is cached on the conn itself.
                return Vec::new();
            } else {
                match conn.list_sessions().await {
                    Ok(infos) => {
                        if let Some(p) = self.state.lock().await.probe_conns.get_mut(&agent_id) {
                            p.at = Instant::now();
                        }
                        return infos;
                    }
                    Err(_) => {
                        // The pooled conn is dead — drop it and fall back
                        // to one fresh spawn below.
                        if let Some(pooled) = self.state.lock().await.probe_conns.remove(&agent_id)
                        {
                            Self::close_agent(pooled.conn).await;
                        }
                    }
                }
            }
        }
        if !Self::probeable(probed.as_ref()) {
            return Vec::new();
        }
        let Ok(conn) = self
            .spawn_agent(
                agent,
                "Failed to spawn agent for lock check",
                &probe_ctx.cwd.to_string_lossy(),
                None,
                None,
            )
            .await
        else {
            return Vec::new();
        };
        // The spawn doubles as the capability probe — an agent that turns
        // out incapable has nothing to list and nothing to pool.
        if !Self::probeable(Some(&conn.capabilities().await)) {
            Self::close_agent(conn).await;
            return Vec::new();
        }
        let Ok(infos) = conn.list_sessions().await else {
            Self::close_agent(conn).await;
            return Vec::new();
        };
        // A racing probe may have pooled a conn first — keep that one and
        // retire ours so no subprocess leaks.
        let raced = {
            let mut state = self.state.lock().await;
            if let std::collections::hash_map::Entry::Vacant(e) = state.probe_conns.entry(agent_id)
            {
                e.insert(ProbeConn {
                    conn: Arc::clone(&conn),
                    at: Instant::now(),
                });
                false
            } else {
                true
            }
        };
        if raced {
            Self::close_agent(conn).await;
        }
        infos
    }

    async fn lock_state(
        &self,
        cwd: &Path,
        force: bool,
        borrowed: Option<(&str, Arc<AcpConnection>)>,
    ) -> LockProbe {
        let ttl = env_duration("SEPIA_LOCK_TTL_MS", DEFAULT_LOCK_TTL);
        if !force {
            let state = self.state.lock().await;
            if let Some((at, probe)) = &state.lock_cache {
                if at.elapsed() < ttl {
                    return probe.clone();
                }
            }
        }
        let probe = self.probe_locks(cwd, ttl, borrowed).await;
        self.state.lock().await.lock_cache = Some((Instant::now(), probe.clone()));
        probe
    }

    fn to_summary(session: &Session) -> SessionSummary {
        let agent = agent_for_backend(&session.backend_type).to_string();
        SessionSummary {
            id: session.id.clone(),
            title: session.title.clone(),
            cwd: session.working_directory.clone(),
            source: agent.clone(),
            agent,
            updated_at: iso_of(session.last_activity_at),
            locked: false,
            lock_holder_pid: None,
            busy: false,
            pinned: None,
            archived: None,
            project_ids: None,
            spans: None,
            parent_session_id: session.parent_session_id.clone(),
            agent_id: session.agent_id.clone(),
        }
    }

    /// Sessions from the IR store; `with_locks` also probes every
    /// registered agent that can answer `session/list` for live lock
    /// state.
    ///
    /// # Errors
    /// `internal` on store failure.
    pub async fn list_sessions(
        &self,
        with_locks: bool,
    ) -> Result<Vec<SessionSummary>, ControlError> {
        let sessions = self
            .repo
            .list()
            .await
            .map_err(storage_error("Failed to list sessions"))?;
        let mut summaries: Vec<SessionSummary> = sessions.iter().map(Self::to_summary).collect();
        let stored: HashSet<String> = summaries.iter().map(|s| s.id.clone()).collect();
        {
            let state = self.state.lock().await;
            for (id, live) in &state.live {
                if !stored.contains(id) {
                    // Pinned to attach time — now() would churn on every
                    // fetch and ride the session to the top.
                    summaries.push(SessionSummary {
                        id: id.clone(),
                        title: live.title.clone(),
                        cwd: live.cwd.clone(),
                        agent: live.agent_id.clone(),
                        updated_at: iso_of(live.attached_at_ms / 1000.0),
                        locked: false,
                        lock_holder_pid: None,
                        source: "sepia".into(),
                        busy: live.busy,
                        pinned: None,
                        archived: None,
                        project_ids: None,
                        spans: None,
                        parent_session_id: None,
                        agent_id: None,
                    });
                }
            }
        }
        if !with_locks {
            return Ok(summaries);
        }
        let probe_cwd = self.probe_cwd.clone();
        let probe = self.lock_state(&probe_cwd, false, None).await;
        for summary in &mut summaries {
            let Some(lock) = probe.merged.get(&summary.id) else {
                continue;
            };
            // `locked` is the union — held in any agent's view counts —
            // but the holder pid prefers the report from the agent owning
            // the session's backend: a pid from a colliding id in another
            // agent's list names a holder that is not holding this session.
            let holder = probe.info_for(&summary.agent, &summary.id);
            summary.locked = lock.locked;
            summary.lock_holder_pid = if lock.locked {
                holder.and_then(|h| h.lock_holder_pid)
            } else {
                None
            };
        }
        Ok(summaries)
    }

    fn history_limit(options: &HistoryOptions) -> usize {
        options
            .limit
            .unwrap_or_else(|| env_number("SEPIA_HISTORY_LIMIT", DEFAULT_HISTORY_LIMIT))
            .max(1)
    }

    /// `ToolCall.arguments` is `Value` — stores keep the parsed arg
    /// object, a few keep the raw JSON string. Re-encode to one JSON
    /// value so the flat row matches the live `args` stream's shape.
    /// `{}` is what adapters record for arg-less calls — left off.
    fn call_args_text(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) if s.is_empty() => None,
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) if o.is_empty() => None,
            other => serde_json::to_string(other).ok(),
        }
    }

    /// `HistoryMessage` projection — shared by the window and paged paths.
    fn project_history(
        slice: &[sepia_core::MessageNode],
        calls_by_id: &HashMap<String, ToolCall>,
    ) -> Vec<HistoryMessage> {
        slice
            .iter()
            .map(|node| {
                let tool_result = &node.tool_result;
                let call = (node.role == Role::Tool)
                    .then(|| node.tool_call_id.as_deref().unwrap_or(""))
                    .and_then(|id| calls_by_id.get(id));
                HistoryMessage {
                    role: node.role,
                    node_id: node.node_id,
                    content: node.content.clone(),
                    blocks: (!node.blocks.is_empty()).then(|| node.blocks.clone()),
                    created_at: node.created_at * 1000.0,
                    tool_name: node.tool_name.clone(),
                    thinking: node.thinking.clone(),
                    thinking_signature: node.thinking_signature.clone(),
                    usage: node.usage.clone(),
                    model: node.model.clone(),
                    request_id: node.request_id.clone(),
                    finish_reason: node.finish_reason.clone(),
                    tool_status: tool_result.as_ref().map(|r| r.status),
                    exit_code: tool_result.as_ref().and_then(|r| r.exit_code),
                    duration_ms: tool_result.as_ref().and_then(|r| r.duration_ms),
                    args: call.and_then(|c| Self::call_args_text(&c.arguments)),
                    locations: call
                        .filter(|c| !c.locations.is_empty())
                        .map(|c| c.locations.clone()),
                    diffs: call
                        .filter(|c| !c.diffs.is_empty())
                        .map(|c| c.diffs.clone()),
                    tool_call_id: (node.role == Role::Tool)
                        .then(|| node.tool_call_id.clone())
                        .flatten(),
                }
            })
            .collect()
    }

    /// Paged session history — the native `nodes_window` path skips
    /// parsing the backlog entirely.
    ///
    /// # Errors
    /// `not_found` on unknown id, `internal` on store failure.
    pub async fn get_history(
        &self,
        id: &str,
        options: &HistoryOptions,
    ) -> Result<HistoryPage, ControlError> {
        let limit = Self::history_limit(options);
        // Fast path: a store that pages natively never parses the backlog —
        // the window plus call-bearing nodes arrive pre-sliced.
        if let Some(win) = self
            .repo
            .nodes_window(
                id,
                &NodesWindowOptions {
                    limit: Some(limit),
                    before: options.before,
                    agent_id: options.agent_id.clone(),
                },
            )
            .await
            .map_err(storage_error("Failed to read session"))?
        {
            let mut calls_by_id = HashMap::new();
            for node in &win.tool_call_nodes {
                for call in &node.tool_calls {
                    calls_by_id.insert(call.id.clone(), call.clone());
                }
            }
            return Ok(HistoryPage {
                messages: Self::project_history(&win.nodes, &calls_by_id),
                total: win.total,
                start: win.start,
            });
        }
        let maybe = self
            .repo
            .get_by_id(id, options.agent_id.as_deref())
            .await
            .map_err(storage_error("Failed to read session"))?;
        let Some(session) = maybe else {
            // A live (attached) session may not exist in the store yet —
            // the agent only flushes it after the first prompt. Treat as
            // empty.
            let state = self.state.lock().await;
            if Self::live_for(&state, id, options.agent_id.as_deref()).is_some() {
                return Ok(HistoryPage {
                    messages: Vec::new(),
                    total: 0,
                    start: 0,
                });
            }
            return Err(control_error(
                ControlErrorCode::NotFound,
                format!("Unknown session: {id}"),
            ));
        };
        let nodes = &session.nodes;
        let total = nodes.len();
        let before = options.before.map_or(total, |b| {
            usize::try_from(b.max(0)).unwrap_or(usize::MAX).min(total)
        });
        let start = before.saturating_sub(limit);
        let slice = &nodes[start..before];
        // Tool rows render the call's file footprint, which lives on the
        // assistant node's `tool_calls` — join by tool_call_id. The map
        // covers the whole backlog so a paged-in slice still resolves
        // its calls.
        let mut calls_by_id = HashMap::new();
        for node in nodes {
            for call in &node.tool_calls {
                calls_by_id.insert(call.id.clone(), call.clone());
            }
        }
        Ok(HistoryPage {
            messages: Self::project_history(slice, &calls_by_id),
            total,
            start,
        })
    }

    /// The complete session IR — nodes, tool calls, thinking, usage.
    ///
    /// # Errors
    /// `not_found` on unknown id.
    pub async fn get_session(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Session, ControlError> {
        self.repo
            .get_by_id(id, agent_id)
            .await
            .map_err(storage_error("Failed to read session"))?
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })
    }

    /// Metadata-only variant — a Session with empty `nodes`/
    /// `prompt_history`, for consumers reading refs/headers that must not
    /// parse a gigabyte-scale backlog.
    ///
    /// # Errors
    /// `not_found` on unknown id.
    pub async fn get_summary(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Session, ControlError> {
        self.repo
            .summary(id, agent_id)
            .await
            .map_err(storage_error("Failed to read session"))?
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })
    }

    /// The default-agent fallback only applies to backends that resolve
    /// to the primary agent anyway — a backend mapped to a concrete agent
    /// id with no registered runtime must fail `unknown_agent`.
    fn agent_for_session(&self, backend_type: &str) -> Option<&Arc<dyn AgentRuntime>> {
        let mapped = agent_for_backend(backend_type);
        if let Some(found) = self.agents.iter().find(|a| a.id() == mapped) {
            return Some(found);
        }
        if mapped == "devin" {
            return self.pick_agent();
        }
        None
    }

    fn pick_agent(&self) -> Option<&Arc<dyn AgentRuntime>> {
        match &self.default_agent_id {
            None => self.agents.first(),
            Some(id) => self.agents.iter().find(|a| a.id() == id.as_str()),
        }
    }

    /// Releases the lock an explicit takeover needs: SIGTERM the reported
    /// holder pid, then poll `session/list` until the lock clears or the
    /// settle window ends. A missing/invalid pid or an already-dead
    /// holder falls through — the load attempt is the arbiter.
    async fn release_lock_holder(&self, conn: &AcpConnection, session_id: &str, pid: Option<f64>) {
        let Some(pid) = pid else { return };
        if pid.fract() != 0.0 || pid <= 0.0 {
            return;
        }
        #[allow(clippy::cast_possible_truncation)]
        let pid = pid as i64;
        #[allow(clippy::cast_possible_wrap)]
        let self_pid = i64::from(std::process::id() as i32);
        if pid == self_pid {
            return;
        }
        // A refused signal (ESRCH — holder already exited) is a signal
        // too; the handler's own errors surface as a panic we contain.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.terminate_lock_holder)(pid);
        }))
        .is_err()
        {
            return;
        }
        // No `session/list` means the release can never be observed — let
        // the load attempt arbitrate instead of polling a missing RPC.
        if !conn.capabilities().await.session_list {
            return;
        }
        let deadline = Instant::now() + TAKEOVER_SETTLE;
        while Instant::now() < deadline {
            tokio::time::sleep(TAKEOVER_POLL).await;
            let current = conn.list_sessions().await.unwrap_or_default();
            if current
                .iter()
                .find(|c| c.session_id == session_id)
                .is_none_or(|c| !c.locked)
            {
                return;
            }
        }
    }

    /// The `prompt_capabilities` flag a non-baseline content block needs
    /// — `text` and `resource_link` are baseline (ACP requires every
    /// agent to take them). Unadvertised flags normalize to false, so a
    /// miss here fails the prompt before the agent errors opaquely
    /// mid-turn.
    async fn disallowed_part(conn: &AcpConnection, parts: &[PromptPart]) -> Option<&'static str> {
        let caps = conn.capabilities().await;
        for part in parts {
            let (allowed, label) = match part {
                PromptPart::Image { .. } => (caps.prompt_capabilities.image, "image"),
                PromptPart::Audio { .. } => (caps.prompt_capabilities.audio, "audio"),
                PromptPart::Resource { .. } => (
                    caps.prompt_capabilities.embedded_context,
                    "embedded context (resource)",
                ),
                _ => continue,
            };
            if !allowed {
                return Some(label);
            }
        }
        None
    }

    /// Wire a fresh `LiveSession`'s forward tasks (updates + permissions
    /// → event fan-out through the shared translator).
    fn wire_live(mut live: LiveSession) -> LiveSession {
        {
            let mut updates = live.conn.updates();
            let events = live.events.clone();
            let translator = Arc::clone(&live.translator);
            live.forward_tasks.push(tokio::spawn(async move {
                while let Ok(update) = updates.recv().await {
                    let translated = translator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .translate(&update);
                    let _ = events.send(translated);
                }
            }));
        }
        {
            let mut permissions = live.conn.permissions();
            let events = live.events.clone();
            live.forward_tasks.push(tokio::spawn(async move {
                while let Ok(request) = permissions.recv().await {
                    let _ = events.send(Translator::permission_request(&request));
                }
            }));
        }
        live
    }

    /// Detach: abort forwards, close the connection.
    async fn unwire_and_close(live: LiveSession) {
        for task in &live.forward_tasks {
            task.abort();
        }
        Arc::clone(&live.conn).close().await;
    }

    fn translator_of(live: &LiveSession) -> std::sync::MutexGuard<'_, Translator> {
        live.translator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn new_live(
        conn: &Arc<AcpConnection>,
        cwd: String,
        title: String,
        agent_id: String,
        thread_id: &str,
    ) -> LiveSession {
        LiveSession {
            conn: Arc::clone(conn),
            events: broadcast::channel(EVENT_BUFFER).0,
            forward_tasks: Vec::new(),
            translator: Arc::new(StdMutex::new(Translator::new(thread_id))),
            cwd,
            title,
            agent_id,
            busy: false,
            idle_since: None,
            attached_at_ms: now_ms(),
        }
    }

    async fn perform_attach(
        self: &Arc<Self>,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent_id: Option<String>,
    ) -> Result<AttachResult, ControlError> {
        // Attach needs only cwd/backend_type/title — a summary read skips
        // parsing a backlog that can reach gigabytes on live stores.
        let session = self
            .repo
            .summary(id, agent_id.as_deref())
            .await
            .map_err(storage_error("Failed to read session"))?
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })?;

        let Some(agent) = self.agent_for_session(&session.backend_type).cloned() else {
            return Err(control_error(
                ControlErrorCode::UnknownAgent,
                format!("No agent available for session: {id}"),
            ));
        };

        let conn = self
            .spawn_agent(
                &agent,
                "Failed to spawn agent",
                &session.working_directory,
                model.as_deref(),
                fallbacks.as_deref(),
            )
            .await?;
        let live = Self::wire_live(Self::new_live(
            &conn,
            session.working_directory.clone(),
            session.title.clone(),
            agent.id().to_string(),
            id,
        ));

        // The lock check fans out to every registered agent — a session a
        // different backend's runtime holds is still seen — with this
        // attach's fresh connection standing in for its own agent's
        // probe, so the holder pid is the one the owning agent reported.
        let cwd = PathBuf::from(&session.working_directory);
        let probe = self
            .lock_state(&cwd, true, Some((agent.id(), Arc::clone(&conn))))
            .await;
        let info = probe.info_for(agent.id(), id).cloned();
        if info.as_ref().is_some_and(|i| i.locked) {
            if !takeover {
                Self::unwire_and_close(live).await;
                return Ok(AttachResult {
                    attached: false,
                    read_only: true,
                    agent_id: agent.id().to_string(),
                    capabilities: conn.capabilities().await,
                });
            }
            // ACP session/load has no steal flag — the only way to take a
            // held session is for the holder to let go: signal the pid the
            // agent itself reported, then let the load decide.
            self.release_lock_holder(&conn, id, info.as_ref().and_then(|i| i.lock_holder_pid))
                .await;
        }

        // `session/load` is a capability, not a baseline method — an
        // agent that never advertised it can only answer
        // method-not-found, so fail with the real reason instead of an
        // opaque load error. A held session still resolved to read-only
        // above.
        if !conn.capabilities().await.load_session {
            Self::unwire_and_close(live).await;
            return Err(control_error(
                ControlErrorCode::Invalid,
                format!("Agent {} does not support session/load: {id}", agent.id()),
            ));
        }

        Self::emit(&live, Self::translator_of(&live).start_run());
        let do_load = || {
            let conn = Arc::clone(&conn);
            let cwd = session.working_directory.clone();
            let id = id.to_string();
            async move { conn.load_session(&id, &cwd).await }
        };
        let mut loaded = do_load().await;
        if loaded.is_err() && takeover {
            tokio::time::sleep(TAKEOVER_RETRY_DELAY).await;
            loaded = do_load().await;
        }
        if let Err(load_err) = loaded {
            Self::emit(&live, Self::translator_of(&live).end_turn());
            Self::unwire_and_close(live).await;
            // A load failure is authoritative: re-probe once, treating a
            // lock as read-only.
            let reprobe = self.lock_state(&cwd, true, None).await;
            let held = reprobe.info_for(agent.id(), id).cloned();
            if held.as_ref().is_some_and(|h| h.locked) {
                // A takeover that still sees the lock held failed —
                // report it instead of quietly degrading to read-only,
                // which callers cannot tell apart from "never tried".
                if takeover {
                    let held_pid = held
                        .as_ref()
                        .and_then(|h| h.lock_holder_pid)
                        .or_else(|| info.and_then(|i| i.lock_holder_pid));
                    return Err(ControlError::caused(
                        ControlErrorCode::Locked,
                        match held_pid {
                            Some(pid) => format!(
                                "Session is held by PID {pid} — it couldn't be released: {id}"
                            ),
                            None => format!(
                                "Session is held by another process — the lock couldn't be released: {id}"
                            ),
                        },
                        format!("{load_err:?}"),
                    ));
                }
                return Ok(AttachResult {
                    attached: false,
                    read_only: true,
                    agent_id: agent.id().to_string(),
                    capabilities: conn.capabilities().await,
                });
            }
            return Err(acp_error("Failed to load session")(load_err));
        }
        Self::emit(&live, Self::translator_of(&live).end_turn());
        let mut live = live;
        Self::touch_idle(&mut live);
        self.state.lock().await.live.insert(id.to_string(), live);
        Ok(AttachResult {
            attached: true,
            read_only: false,
            agent_id: agent.id().to_string(),
            capabilities: conn.capabilities().await,
        })
    }

    /// Spawns the session's agent and loads the session. Locked sessions
    /// attach read-only unless `takeover` signals the reported lock-holder
    /// pid first; a takeover that still cannot load fails `locked`. An
    /// agent that never advertised `load_session` fails `invalid`.
    ///
    /// # Errors
    /// `not_found`/`unknown_agent`/`invalid`/`locked`/`internal`/`conflict`.
    pub async fn attach(
        self: &Arc<Self>,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent_id: Option<String>,
    ) -> Result<AttachResult, ControlError> {
        {
            let state = self.state.lock().await;
            if let Some(existing) = state.live.get(id) {
                if agent_id.is_none() || Some(existing.agent_id.as_str()) == agent_id.as_deref() {
                    return Ok(AttachResult {
                        attached: true,
                        read_only: false,
                        agent_id: existing.agent_id.clone(),
                        capabilities: existing.conn.capabilities().await,
                    });
                }
                // The id is held by another agent's live session; the map
                // is keyed by bare id and cannot host both copies.
                return Err(control_error(
                    ControlErrorCode::Conflict,
                    format!("Session is already attached under a different agent: {id}"),
                ));
            }
        }

        // Claim the id before the first await so concurrent attaches
        // share one spawn. The token guards eviction — a newer pending
        // attach must not be removed by an older one finishing.
        let token = self.attach_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let pending: AttachFuture = {
            let mut state = self.state.lock().await;
            if let Some((_, shared)) = state.pending_attaches.get(id) {
                shared.clone()
            } else {
                let plane = Arc::clone(self);
                let id_owned = id.to_string();
                let fut: BoxFuture<'static, Result<AttachResult, ControlError>> =
                    Box::pin(async move {
                        plane
                            .perform_attach(&id_owned, takeover, model, fallbacks, agent_id)
                            .await
                    });
                let shared = fut.shared();
                state
                    .pending_attaches
                    .insert(id.to_string(), (token, shared.clone()));
                shared
            }
        };
        let outcome = pending.await;
        {
            let mut state = self.state.lock().await;
            if matches!(state.pending_attaches.get(id), Some((t, _)) if *t == token) {
                state.pending_attaches.remove(id);
            }
        }
        outcome
    }

    /// Spawns an agent, creates a fresh session, and registers it live so
    /// it can be prompted immediately.
    ///
    /// # Errors
    /// `invalid`/`unknown_agent`/`internal`.
    pub async fn create_session(
        self: &Arc<Self>,
        cwd: &str,
        agent_id: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError> {
        if cwd.trim().is_empty() || !Path::new(cwd).is_absolute() {
            return Err(control_error(
                ControlErrorCode::Invalid,
                format!("cwd must be a non-empty absolute path: {cwd}"),
            ));
        }
        let agent = match &agent_id {
            None => self.pick_agent(),
            Some(id) => self.agents.iter().find(|a| a.id() == id.as_str()),
        };
        let Some(agent) = agent.cloned() else {
            return Err(control_error(
                ControlErrorCode::UnknownAgent,
                format!(
                    "Unknown agent: {}",
                    agent_id.unwrap_or_else(|| self.default_agent_id.clone().unwrap_or_default())
                ),
            ));
        };

        let conn = self
            .spawn_agent(
                &agent,
                "Failed to spawn agent",
                cwd,
                model.as_deref(),
                fallbacks.as_deref(),
            )
            .await?;
        let id = match conn.new_session(cwd).await {
            Ok(id) => id,
            Err(e) => {
                Self::close_agent(conn).await;
                return Err(acp_error("Failed to create session")(e));
            }
        };
        let mut live = Self::wire_live(Self::new_live(
            &conn,
            cwd.to_string(),
            title.unwrap_or_else(|| "New session".into()),
            agent.id().to_string(),
            &id,
        ));
        Self::touch_idle(&mut live);
        self.state.lock().await.live.insert(id.clone(), live);
        Ok(CreateResult {
            id,
            agent_id: agent.id().to_string(),
            capabilities: conn.capabilities().await,
        })
    }

    /// Detach a live session (no-op when not attached).
    pub async fn detach(&self, id: &str) {
        let live = self.state.lock().await.live.remove(id);
        if let Some(live) = live {
            Self::unwire_and_close(live).await;
        }
    }

    fn require_live<'a>(
        state: &'a State,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<&'a LiveSession, ControlError> {
        Self::live_for(state, id, agent_id).ok_or_else(|| {
            control_error(
                ControlErrorCode::Invalid,
                format!("Session is not attached: {id}"),
            )
        })
    }

    /// Sends one turn — `parts` is the ACP `session/prompt` content-block
    /// list, forwarded verbatim after the capability gate.
    ///
    /// # Errors
    /// `invalid`/`busy`/`internal`.
    pub async fn prompt(
        &self,
        id: &str,
        parts: &[PromptPart],
        agent_id: Option<&str>,
    ) -> Result<(), ControlError> {
        let conn = {
            let state = self.state.lock().await;
            let live = Self::require_live(&state, id, agent_id)?;
            Arc::clone(&live.conn)
        };
        if let Some(label) = Self::disallowed_part(&conn, parts).await {
            return Err(control_error(
                ControlErrorCode::Invalid,
                format!("Agent does not accept {label} prompt content: {id}"),
            ));
        }
        {
            let mut state = self.state.lock().await;
            let Some(live) = Self::live_for(&state, id, agent_id).map(|l| l.agent_id.clone())
            else {
                return Err(control_error(
                    ControlErrorCode::Invalid,
                    format!("Session is not attached: {id}"),
                ));
            };
            let _ = live;
            let Some(live) = state.live.get_mut(id) else {
                return Err(control_error(
                    ControlErrorCode::Invalid,
                    format!("Session is not attached: {id}"),
                ));
            };
            if live.busy {
                return Err(control_error(
                    ControlErrorCode::Busy,
                    format!("Session is busy: {id}"),
                ));
            }
            live.busy = true;
            live.idle_since = None;
            let start = Self::translator_of(live).start_run();
            Self::emit(live, start);
        }
        let result = conn
            .prompt(id, parts)
            .await
            .map_err(acp_error("Failed to send prompt"));
        {
            let mut state = self.state.lock().await;
            if let Some(live) = state.live.get_mut(id) {
                live.busy = false;
                Self::touch_idle(live);
                let end = Self::translator_of(live).end_turn();
                Self::emit(live, end);
            }
        }
        result
    }

    /// `session/cancel` — fire-and-forget turn cancellation.
    ///
    /// # Errors
    /// `invalid`/`internal`.
    pub async fn cancel(&self, id: &str, agent_id: Option<&str>) -> Result<(), ControlError> {
        let conn = {
            let state = self.state.lock().await;
            let live = Self::require_live(&state, id, agent_id)?;
            Arc::clone(&live.conn)
        };
        conn.cancel(id)
            .await
            .map_err(acp_error("Failed to cancel prompt"))
    }

    /// Detaches if live, then deletes the session through its agent
    /// runtime. Fails `invalid` when the agent never advertised
    /// `sessionCapabilities.delete`.
    ///
    /// # Errors
    /// `not_found`/`unknown_agent`/`invalid`/`internal`.
    pub async fn delete_session(
        self: &Arc<Self>,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<(), ControlError> {
        // A live handle keeps the session open inside the agent; drop
        // ours first so our own lock does not make the delete fail. A
        // live entry for a different agent's colliding id is left alone.
        let live = {
            let mut state = self.state.lock().await;
            if Self::live_for(&state, id, agent_id).is_some() {
                state.live.remove(id)
            } else {
                None
            }
        };
        let (live_cwd, live_agent) = match live {
            Some(live) => {
                let pair = (live.cwd.clone(), live.agent_id.clone());
                Self::unwire_and_close(live).await;
                (Some(pair.0), Some(pair.1))
            }
            None => (None, None),
        };

        let maybe = self
            .repo
            .summary(id, agent_id)
            .await
            .map_err(storage_error("Failed to read session"))?;
        let cwd = live_cwd
            .or_else(|| maybe.as_ref().map(|s| s.working_directory.clone()))
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })?;
        let resolved_agent = agent_id.map(str::to_string).or(live_agent).or_else(|| {
            maybe
                .as_ref()
                .map(|s| agent_for_backend(&s.backend_type).to_string())
        });
        let agent = resolved_agent
            .as_deref()
            .and_then(|rid| self.agents.iter().find(|a| a.id() == rid))
            .or_else(|| {
                (resolved_agent.is_none() || resolved_agent.as_deref() == Some("devin"))
                    .then(|| self.pick_agent())
                    .flatten()
            });
        let Some(agent) = agent.cloned() else {
            return Err(control_error(
                ControlErrorCode::UnknownAgent,
                format!("No agent available for session: {id}"),
            ));
        };

        let conn = self
            .spawn_agent(&agent, "Failed to spawn agent", &cwd, None, None)
            .await?;
        // `session/delete` is a capability — a non-advertising agent would
        // only answer method-not-found; fail with the real reason. The
        // spawn that learned this is the fresh advertisement.
        if !conn.capabilities().await.session_capabilities.delete {
            Self::close_agent(conn).await;
            return Err(control_error(
                ControlErrorCode::Invalid,
                format!("Agent {} does not support session/delete: {id}", agent.id()),
            ));
        }
        let result = conn
            .delete_session(id)
            .await
            .map_err(acp_error("Failed to delete session"));
        Self::close_agent(conn).await;
        result
    }

    /// Settles a pending permission request.
    ///
    /// # Errors
    /// `invalid` when not attached, `not_found` on unknown request id.
    pub async fn respond_to_permission(
        &self,
        id: &str,
        request_id: &str,
        option_id: Option<&str>,
        agent_id: Option<&str>,
    ) -> Result<(), ControlError> {
        let conn = {
            let state = self.state.lock().await;
            let live = Self::require_live(&state, id, agent_id)?;
            Arc::clone(&live.conn)
        };
        if !conn.respond_to_permission(request_id, option_id) {
            return Err(control_error(
                ControlErrorCode::NotFound,
                format!("Unknown permission request: {request_id}"),
            ));
        }
        Ok(())
    }

    /* ---- restore -----------------------------------------------------
     * File restore only — the IR has no deletion model. Three sources,
     * all gated on `confirm` and refused while busy or held by a live
     * process: path restore (diff reversal), checkpoint restore
     * (shadow-git `ref^..ref`), file-history restore (recorded
     * path→backup map; `null` backups are deletion tombstones).
     */

    /// Revert `path` through the session's recorded diffs.
    async fn restore_path(
        &self,
        session: &Session,
        path: &str,
        tool_call_id: Option<&str>,
    ) -> Result<RestoreResult, ControlError> {
        let Some(abs) = restore::resolve_workspace_path(&session.working_directory, path) else {
            return Err(control_error(
                ControlErrorCode::Invalid,
                format!("path must resolve inside the session working directory: {path}"),
            ));
        };
        let raw = self.restore_exec.read_file(&abs).await.map_err(|e| {
            ControlError::caused(
                ControlErrorCode::Internal,
                format!("Failed to read {}", abs.display()),
                e,
            )
        })?;
        let plan = restore::plan_path_restore(
            session,
            path,
            raw.as_deref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .as_deref(),
            tool_call_id,
        );
        match plan {
            FileRestorePlan::Skip { path, reason } => Ok(RestoreResult {
                restored: Vec::new(),
                skipped: vec![SkippedFile { path, reason }],
            }),
            FileRestorePlan::Unchanged { .. } => Ok(RestoreResult {
                restored: vec![RestoredFile {
                    path: abs.display().to_string(),
                    action: RestoreAction::Unchanged,
                    bytes: None,
                }],
                skipped: Vec::new(),
            }),
            FileRestorePlan::Delete { .. } => {
                self.restore_exec.remove_file(&abs).await.map_err(|e| {
                    ControlError::caused(
                        ControlErrorCode::Internal,
                        format!("Failed to delete {}", abs.display()),
                        e,
                    )
                })?;
                Ok(RestoreResult {
                    restored: vec![RestoredFile {
                        path: abs.display().to_string(),
                        action: RestoreAction::Deleted,
                        bytes: None,
                    }],
                    skipped: Vec::new(),
                })
            }
            FileRestorePlan::Write { content, .. } => {
                let bytes = content.as_bytes();
                self.restore_exec
                    .write_file(&abs, bytes)
                    .await
                    .map_err(|e| {
                        ControlError::caused(
                            ControlErrorCode::Internal,
                            format!("Failed to write {}", abs.display()),
                            e,
                        )
                    })?;
                Ok(RestoreResult {
                    restored: vec![RestoredFile {
                        path: abs.display().to_string(),
                        action: RestoreAction::Written,
                        bytes: Some(bytes.len()),
                    }],
                    skipped: Vec::new(),
                })
            }
        }
    }

    /// Store-recorded ids/names become path segments — keep them single
    /// safe file names.
    fn is_safe_file_name(name: &str) -> bool {
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
    }

    /// Materialize a Claude `file-history-snapshot` checkpoint: copy each
    /// tracked file's backup blob out of `<file_history_dir>/<session_id>/`
    /// into the workspace path it covers. `backup: None` is the deletion
    /// tombstone — the file did not exist at that checkpoint. A ref with
    /// no recorded map fails `conflict`.
    async fn restore_file_history(
        &self,
        session: &Session,
        ref_: &str,
        paths: Option<&[String]>,
    ) -> Result<RestoreResult, ControlError> {
        let Some(snapshot) = restore::file_history_snapshot(session, ref_)
            .filter(|s| Self::is_safe_file_name(&s.session_id))
        else {
            return Err(control_error(
                ControlErrorCode::Conflict,
                format!(
                    "Checkpoint {ref_} carries no file map — restore is not supported for this store"
                ),
            ));
        };
        let backup_dir = self.file_history_dir.join(&snapshot.session_id);
        let mut covered = Vec::new();
        let mut skipped = Vec::new();
        for (tracked, info) in &snapshot.files {
            match restore::resolve_workspace_path(&session.working_directory, tracked) {
                None => skipped.push(SkippedFile {
                    path: tracked.clone(),
                    reason: "outside the session working directory".into(),
                }),
                Some(abs) => covered.push(CoveredFile {
                    tracked: tracked.clone(),
                    abs,
                    backup: info.backup.clone(),
                }),
            }
        }

        let mut targets: Vec<&CoveredFile> = Vec::new();
        match paths {
            None => targets.extend(covered.iter()),
            Some(paths) => {
                for p in paths {
                    let Some(abs) = restore::resolve_workspace_path(&session.working_directory, p)
                    else {
                        return Err(control_error(
                            ControlErrorCode::Invalid,
                            format!(
                                "paths entries must resolve inside the session working directory: {p}"
                            ),
                        ));
                    };
                    match covered.iter().find(|entry| entry.abs == abs) {
                        None => skipped.push(SkippedFile {
                            path: p.clone(),
                            reason: format!("not touched by checkpoint {ref_}"),
                        }),
                        Some(hit) => targets.push(hit),
                    }
                }
            }
        }

        let mut restored = Vec::new();
        for target in targets {
            let existing = self
                .restore_exec
                .read_file(&target.abs)
                .await
                .map_err(|e| {
                    ControlError::caused(
                        ControlErrorCode::Internal,
                        format!("Failed to read {}", target.abs.display()),
                        e,
                    )
                })?;
            let Some(backup_name) = &target.backup else {
                // Tombstone — the file did not exist at that checkpoint.
                if existing.is_none() {
                    restored.push(RestoredFile {
                        path: target.abs.display().to_string(),
                        action: RestoreAction::Unchanged,
                        bytes: None,
                    });
                } else {
                    self.restore_exec
                        .remove_file(&target.abs)
                        .await
                        .map_err(|e| {
                            ControlError::caused(
                                ControlErrorCode::Internal,
                                format!("Failed to delete {}", target.abs.display()),
                                e,
                            )
                        })?;
                    restored.push(RestoredFile {
                        path: target.abs.display().to_string(),
                        action: RestoreAction::Deleted,
                        bytes: None,
                    });
                }
                continue;
            };
            if !Self::is_safe_file_name(backup_name) {
                skipped.push(SkippedFile {
                    path: target.tracked.clone(),
                    reason: "unsafe backup name recorded".into(),
                });
                continue;
            }
            let data = self
                .restore_exec
                .read_file(&backup_dir.join(backup_name))
                .await
                .map_err(|e| {
                    ControlError::caused(
                        ControlErrorCode::Internal,
                        "Failed to read file-history backup",
                        e,
                    )
                })?;
            let Some(data) = data else {
                skipped.push(SkippedFile {
                    path: target.tracked.clone(),
                    reason: format!("backup missing from the file-history store: {backup_name}"),
                });
                continue;
            };
            if existing.as_deref() == Some(data.as_slice()) {
                restored.push(RestoredFile {
                    path: target.abs.display().to_string(),
                    action: RestoreAction::Unchanged,
                    bytes: None,
                });
                continue;
            }
            self.restore_exec
                .write_file(&target.abs, &data)
                .await
                .map_err(|e| {
                    ControlError::caused(
                        ControlErrorCode::Internal,
                        format!("Failed to write {}", target.abs.display()),
                        e,
                    )
                })?;
            restored.push(RestoredFile {
                path: target.abs.display().to_string(),
                action: RestoreAction::Written,
                bytes: Some(data.len()),
            });
        }
        Ok(RestoreResult { restored, skipped })
    }

    /// Materialize the files a checkpoint covers. Shadow-git refs use the
    /// `ref^..ref` name list (a stash ref's base is its first parent; a
    /// root commit's is its whole tree); `file-history-snapshot` refs
    /// materialize the recorded path→backup map instead. Files absent at
    /// the ref get deleted — the checkpoint recorded them as removed.
    async fn restore_checkpoint(
        &self,
        session: &Session,
        ref_: &str,
        paths: Option<&[String]>,
    ) -> Result<RestoreResult, ControlError> {
        let Some(entry) = session.checkpoints.iter().find(|c| c.r#ref == ref_) else {
            return Err(control_error(
                ControlErrorCode::NotFound,
                format!("Unknown checkpoint ref: {ref_}"),
            ));
        };
        if entry.kind.as_deref() == Some(restore::FILE_HISTORY_KIND) {
            return self.restore_file_history(session, ref_, paths).await;
        }
        let cwd = PathBuf::from(&session.working_directory);
        let git_err =
            |e: String| ControlError::caused(ControlErrorCode::Internal, "git probe failed", e);
        let inside = self
            .restore_exec
            .git(&cwd, &["rev-parse", "--is-inside-work-tree"])
            .await
            .map_err(git_err)?;
        if inside.code != 0 || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
            return Err(control_error(
                ControlErrorCode::Conflict,
                format!(
                    "Session working directory is not a git work tree: {}",
                    session.working_directory
                ),
            ));
        }
        let object = self
            .restore_exec
            .git(&cwd, &["cat-file", "-e", &format!("{ref_}^{{commit}}")])
            .await
            .map_err(git_err)?;
        if object.code != 0 {
            return Err(control_error(
                ControlErrorCode::Conflict,
                format!("Checkpoint {ref_} is not present in the workspace repository"),
            ));
        }
        let root_result = self
            .restore_exec
            .git(&cwd, &["rev-parse", "--show-toplevel"])
            .await
            .map_err(git_err)?;
        if root_result.code != 0 {
            return Err(control_error(
                ControlErrorCode::Internal,
                format!("git rev-parse failed: {}", root_result.stderr.trim()),
            ));
        }
        let root = String::from_utf8_lossy(&root_result.stdout)
            .trim()
            .to_string();
        let parents = self
            .restore_exec
            .git(&cwd, &["rev-list", "--parents", "-n", "1", ref_])
            .await
            .map_err(git_err)?;
        let base = String::from_utf8_lossy(&parents.stdout)
            .split_whitespace()
            .nth(1)
            .map(str::to_string);
        let covered_result = match &base {
            None => {
                self.restore_exec
                    .git(&cwd, &["ls-tree", "-r", "--name-only", "-z", ref_])
                    .await
            }
            Some(base) => {
                self.restore_exec
                    .git(&cwd, &["diff", "--name-only", "-z", base, ref_])
                    .await
            }
        }
        .map_err(|e| {
            ControlError::caused(
                ControlErrorCode::Internal,
                format!("Failed to list files covered by checkpoint {ref_}"),
                e,
            )
        })?;
        if covered_result.code != 0 {
            return Err(control_error(
                ControlErrorCode::Internal,
                format!(
                    "git diff failed for checkpoint {ref_}: {}",
                    covered_result.stderr.trim()
                ),
            ));
        }
        let covered: Vec<String> = String::from_utf8_lossy(&covered_result.stdout)
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();

        // Git paths are repo-root relative; a restore only writes inside
        // the session's working directory.
        let to_abs = |rel: &str| Path::new(&root).join(rel);
        let in_cwd = |abs: &Path| {
            restore::resolve_workspace_path(&session.working_directory, &abs.to_string_lossy())
                .is_some()
        };

        let mut restored = Vec::new();
        let mut skipped = Vec::new();
        let targets: Vec<String> = match paths {
            None => covered.clone(),
            Some(paths) => {
                let covered_set: HashSet<&str> = covered.iter().map(String::as_str).collect();
                let mut requested = Vec::new();
                for p in paths {
                    let Some(abs) = restore::resolve_workspace_path(&session.working_directory, p)
                    else {
                        return Err(control_error(
                            ControlErrorCode::Invalid,
                            format!(
                                "paths entries must resolve inside the session working directory: {p}"
                            ),
                        ));
                    };
                    let rel = abs.strip_prefix(&root).map_or_else(
                        |_| abs.to_string_lossy().to_string(),
                        |r| r.to_string_lossy().to_string(),
                    );
                    if covered_set.contains(rel.as_str()) {
                        requested.push(rel);
                    } else {
                        skipped.push(SkippedFile {
                            path: p.clone(),
                            reason: format!("not touched by checkpoint {ref_}"),
                        });
                    }
                }
                requested
            }
        };

        for rel in targets {
            let abs = to_abs(&rel);
            if !in_cwd(&abs) {
                skipped.push(SkippedFile {
                    path: rel,
                    reason: "outside the session working directory".into(),
                });
                continue;
            }
            let present = self
                .restore_exec
                .git(&cwd, &["cat-file", "-e", &format!("{ref_}:{rel}")])
                .await
                .map_err(git_err)?;
            let existing = self.restore_exec.read_file(&abs).await.map_err(|e| {
                ControlError::caused(
                    ControlErrorCode::Internal,
                    format!("Failed to read {}", abs.display()),
                    e,
                )
            })?;
            if present.code == 0 {
                let blob = self
                    .restore_exec
                    .git(&cwd, &["show", &format!("{ref_}:{rel}")])
                    .await
                    .map_err(|e| {
                        ControlError::caused(
                            ControlErrorCode::Internal,
                            format!("Failed to read {rel}"),
                            e,
                        )
                    })?;
                if blob.code != 0 {
                    skipped.push(SkippedFile {
                        path: rel,
                        reason: format!("git show failed: {}", blob.stderr.trim()),
                    });
                    continue;
                }
                if existing.as_deref() == Some(blob.stdout.as_slice()) {
                    restored.push(RestoredFile {
                        path: abs.display().to_string(),
                        action: RestoreAction::Unchanged,
                        bytes: None,
                    });
                    continue;
                }
                self.restore_exec
                    .write_file(&abs, &blob.stdout)
                    .await
                    .map_err(|e| {
                        ControlError::caused(
                            ControlErrorCode::Internal,
                            format!("Failed to write {}", abs.display()),
                            e,
                        )
                    })?;
                restored.push(RestoredFile {
                    path: abs.display().to_string(),
                    action: RestoreAction::Written,
                    bytes: Some(blob.stdout.len()),
                });
            } else if existing.is_none() {
                restored.push(RestoredFile {
                    path: abs.display().to_string(),
                    action: RestoreAction::Unchanged,
                    bytes: None,
                });
            } else {
                self.restore_exec.remove_file(&abs).await.map_err(|e| {
                    ControlError::caused(
                        ControlErrorCode::Internal,
                        format!("Failed to delete {}", abs.display()),
                        e,
                    )
                })?;
                restored.push(RestoredFile {
                    path: abs.display().to_string(),
                    action: RestoreAction::Deleted,
                    bytes: None,
                });
            }
        }
        Ok(RestoreResult { restored, skipped })
    }

    /// Refused while the session is busy or locked by a live process.
    async fn guard_session_writes(
        &self,
        id: &str,
        session: &Session,
        agent_id: Option<&str>,
    ) -> Result<bool, ControlError> {
        let live_busy = {
            let state = self.state.lock().await;
            Self::live_for(&state, id, agent_id).map(|l| l.busy)
        };
        if live_busy == Some(true) {
            return Err(control_error(
                ControlErrorCode::Busy,
                format!("Session is busy — wait for the run to finish: {id}"),
            ));
        }
        // An unattached session can still be held by another process —
        // the same lock rule attach enforces. Our own live attach is the
        // holder we're checking through, so only probe when nothing is
        // live here.
        if live_busy.is_none() {
            let probe = self
                .lock_state(&PathBuf::from(&session.working_directory), false, None)
                .await;
            let lock = probe
                .info_for(agent_for_backend(&session.backend_type), id)
                .cloned();
            if lock.as_ref().is_some_and(|l| l.locked) {
                return Err(control_error(
                    ControlErrorCode::Locked,
                    match lock.and_then(|l| l.lock_holder_pid) {
                        Some(pid) => format!("Session is held by PID {pid}: {id}"),
                        None => format!("Session is held by another process: {id}"),
                    },
                ));
            }
        }
        Ok(live_busy.is_some())
    }

    /// Restores files under the session's working directory — either
    /// reverse-applying recorded `ToolCall.diffs` for a `path` or
    /// materializing a `Session.checkpoints` ref. Refused while busy or
    /// locked by a live process; requires `confirm`.
    ///
    /// # Errors
    /// `invalid`/`not_found`/`busy`/`locked`/`conflict`/`internal`.
    pub async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent_id: Option<&str>,
    ) -> Result<RestoreResult, ControlError> {
        if !request.confirm {
            return Err(control_error(
                ControlErrorCode::Invalid,
                "Restore writes files — pass confirm: true",
            ));
        }
        let has_path = request.path.is_some();
        let has_checkpoint = request.checkpoint.is_some();
        if has_path == has_checkpoint {
            return Err(control_error(
                ControlErrorCode::Invalid,
                "restore needs exactly one of path or checkpoint",
            ));
        }
        let session = self
            .repo
            .get_by_id(id, agent_id)
            .await
            .map_err(storage_error("Failed to read session"))?
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })?;
        self.guard_session_writes(id, &session, agent_id).await?;
        if has_checkpoint {
            self.restore_checkpoint(
                &session,
                request.checkpoint.as_deref().unwrap_or_default(),
                request.paths.as_deref(),
            )
            .await
        } else {
            self.restore_path(
                &session,
                request.path.as_deref().unwrap_or_default(),
                request.tool_call_id.as_deref(),
            )
            .await
        }
    }

    /// Conversation rewind — truncates the session's transcript through
    /// the backend's `SessionRewinder` (a backend without one fails
    /// `conflict`). Refused while busy or locked; a live idle attach is
    /// detached first, since the agent's in-memory copy would reflush
    /// the deleted tail. Requires `confirm`.
    ///
    /// # Errors
    /// `invalid`/`not_found`/`busy`/`locked`/`conflict`/`internal`.
    pub async fn rewind(
        self: &Arc<Self>,
        id: &str,
        request: &RewindRequest,
        agent_id: Option<&str>,
    ) -> Result<RewindResult, ControlError> {
        if !request.confirm {
            return Err(control_error(
                ControlErrorCode::Invalid,
                "Rewind deletes stored history — pass confirm: true",
            ));
        }
        let session = self
            .repo
            .get_by_id(id, agent_id)
            .await
            .map_err(storage_error("Failed to read session"))?
            .ok_or_else(|| {
                control_error(ControlErrorCode::NotFound, format!("Unknown session: {id}"))
            })?;
        let was_live = self.guard_session_writes(id, &session, agent_id).await?;

        let target = match (request.node_id, request.turns, request.checkpoint.clone()) {
            (Some(node_id), None, None) => RewindTarget::NodeId(node_id),
            (None, Some(turns), None) => RewindTarget::Turns(turns),
            (None, None, Some(checkpoint)) => RewindTarget::Checkpoint(checkpoint),
            _ => {
                return Err(control_error(
                    ControlErrorCode::Invalid,
                    "rewind needs exactly one of nodeId, turns, or checkpoint",
                ));
            }
        };
        let plan = rewind::plan_rewind(&session, &target)
            .map_err(|reason| control_error(ControlErrorCode::Invalid, reason))?;
        // Already at the requested point — nothing to write.
        if plan.removed.is_empty() {
            return Ok(RewindResult {
                kept: plan.keep_count,
                removed: 0,
            });
        }

        let Some(rewinder) = self
            .rewinders
            .get(agent_for_backend(&session.backend_type))
            .cloned()
        else {
            return Err(control_error(
                ControlErrorCode::Conflict,
                format!(
                    "Rewind is not supported for this store: {}",
                    session.backend_type
                ),
            ));
        };
        // A live agent holds the pre-rewind transcript in memory and
        // flushes it back on the next turn — drop the attach before
        // writing.
        if was_live {
            self.detach(id).await;
        }
        let truncated = rewind::rewind_session(&session, &plan);
        rewinder.truncate(&session, &plan, &truncated).await?;
        Ok(RewindResult {
            kept: plan.keep_count,
            removed: plan.removed.len(),
        })
    }

    /// Subscribe to a live session's event stream — dropping the receiver
    /// unsubscribes.
    ///
    /// # Errors
    /// `invalid` when the session isn't attached.
    pub async fn subscribe(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError> {
        let mut state = self.state.lock().await;
        Self::require_live(&state, id, agent_id)?;
        let live = state.live.get_mut(id).ok_or_else(|| {
            control_error(
                ControlErrorCode::Invalid,
                format!("Session is not attached: {id}"),
            )
        })?;
        live.idle_since = None;
        Ok(live.events.subscribe())
    }

    /// Every registered agent + its last probed capability advertisement
    /// (`None` until the first spawn probes).
    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let state = self.state.lock().await;
        self.agents
            .iter()
            .map(|agent| AgentInfo {
                id: agent.id().to_string(),
                label: agent.label().to_string(),
                capabilities: state.probed_capabilities.get(agent.id()).cloned(),
            })
            .collect()
    }

    /// A live session nobody is listening to still owns an agent
    /// subprocess; reclaim it once it has been idle (no listeners, no
    /// in-flight turn) for the TTL.
    async fn sweep_idle(&self) {
        let Some(ttl) = self.idle_ttl else { return };
        let expired: Vec<String> = {
            let state = self.state.lock().await;
            let now = Instant::now();
            state
                .live
                .iter()
                .filter(|(_, live)| {
                    !live.busy
                        && live.events.receiver_count() == 0
                        && live
                            .idle_since
                            .is_some_and(|t| now.duration_since(t) >= ttl)
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in expired {
            self.detach(&id).await;
        }
    }

    /// Retire every live session and pooled probe connection — pooled
    /// conns aren't live sessions, so `detach` never reaches them.
    pub async fn close_all(&self) {
        self.stop_sweep.store(true, Ordering::SeqCst);
        let (lives, probes) = {
            let mut state = self.state.lock().await;
            (
                state.live.drain().map(|(_, l)| l).collect::<Vec<_>>(),
                state
                    .probe_conns
                    .drain()
                    .map(|(_, p)| p)
                    .collect::<Vec<_>>(),
            )
        };
        for live in lives {
            Self::unwire_and_close(live).await;
        }
        for probe in probes {
            Self::close_agent(probe.conn).await;
        }
    }
}

/// The spawn result of `create_session`.
#[derive(Clone, Debug)]
pub struct CreateResult {
    pub id: String,
    pub agent_id: String,
    pub capabilities: AcpCapabilities,
}

fn iso_of(epoch_secs: f64) -> String {
    let Ok(dt) = time::OffsetDateTime::from_unix_timestamp_nanos((epoch_secs * 1e9) as i128) else {
        return String::new();
    };
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// SIGTERM (graceful — the holder is usually an agent TUI on this
/// machine), never SIGKILL, never an unreported or guessed pid.
fn default_terminate(pid: i64) {
    let Ok(raw) = i32::try_from(pid) else { return };
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(raw),
        nix::sys::signal::Signal::SIGTERM,
    );
}
