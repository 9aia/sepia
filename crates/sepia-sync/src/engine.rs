//! The sync engine — one supervised loop per node, `JoinSet`-managed,
//! `CancellationToken`-cancellable.
//!
//! Per node the cycle is: `list_sessions` → upsert + reconcile the
//! projection → drain the outbox FIFO → tail `/api/events` and apply
//! summary diffs → on stream end/error, mark the node down and
//! reconnect with capped exponential backoff + jitter. A periodic
//! full-list reconcile catches anything the feed missed (tombstones are
//! only visible in listings).

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sepia_outbox::{Outbox, Status as OutboxStatus};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::client::{ClientConfig, FeedEvent, NodeClient, NodeError};
use crate::{NodeRef, NodeStatus, NodeWrite, PatchStatus, ProjectionStore, SyncError};

/// Tuning knobs — production defaults; tests shrink the sleeps.
#[derive(Clone, Copy, Debug)]
pub struct SyncOptions {
    /// TCP connect budget for node calls.
    pub connect_timeout: Duration,
    /// End-to-end budget for REST calls (list, post, refetch).
    pub request_timeout: Duration,
    /// Max lifetime of one SSE subscription — the stream is reconnected
    /// (and the index re-listed) when it lapses. Also bounds how long a
    /// dead-silent SSE read can outlive engine shutdown.
    pub sse_max_age: Duration,
    /// First reconnect delay.
    pub backoff_min: Duration,
    /// Reconnect-delay cap.
    pub backoff_max: Duration,
    /// Periodic full `list_sessions` reconcile while subscribed.
    pub resync_interval: Duration,
    /// Capacity of the per-node SSE event channel.
    pub feed_channel: usize,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(15),
            sse_max_age: Duration::from_secs(60),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            resync_interval: Duration::from_secs(300),
            feed_channel: 256,
        }
    }
}

impl SyncOptions {
    fn client_config(&self) -> ClientConfig {
        ClientConfig {
            connect_timeout: self.connect_timeout,
            request_timeout: self.request_timeout,
            sse_max_age: self.sse_max_age,
        }
    }
}

/// A node registration — the [`NodeRef`] plus its bearer credential.
#[derive(Clone, Debug)]
pub struct NodeTarget {
    /// The node.
    pub node: NodeRef,
    /// Bearer token minted by `POST /api/pair` on that node.
    pub token: Option<String>,
}

impl NodeTarget {
    pub fn new(node: NodeRef) -> Self {
        Self { node, token: None }
    }

    pub fn with_token(node: NodeRef, token: impl Into<String>) -> Self {
        Self {
            node,
            token: Some(token.into()),
        }
    }
}

/// What [`SyncHandle::submit_op`] did with a write.
#[derive(Clone, Debug)]
pub enum SubmitOutcome {
    /// The node was up and accepted the write — the response body.
    Applied(Value),
    /// The node is down (or the write hit a transport error) — queued
    /// for replay on reconnect. `Box` keeps the enum small.
    Queued(Box<sepia_outbox::OutboxEntry>),
}

/// Shared state between the supervisor, the per-node loops, and the
/// public handle.
struct Shared {
    projection: ProjectionStore,
    outbox: Outbox,
    options: SyncOptions,
    /// Registered nodes — also the source of per-node bearer tokens for
    /// `submit_op`'s direct path.
    targets: Mutex<HashMap<String, NodeTarget>>,
    /// Live SSE pump threads — exposed via `SyncHandle::active_pumps`
    /// so shutdown/teardown can be observed, never blocked on.
    active_pumps: Arc<AtomicUsize>,
}

/// Messages the SSE pump thread sends back to the async node loop.
enum PumpMsg {
    /// `GET /api/events` returned 2xx — the node has registered the
    /// broadcast subscription. Always the first message on success so
    /// the loop can order "subscribe → list" without a diff gap.
    Subscribed,
    Event(FeedEvent),
    /// EOF or read error — the connection is over either way; the
    /// payload is `None` for a clean close.
    Closed(Option<NodeError>),
}

/// The entry point: spawns the supervisor and returns a [`SyncHandle`].
pub struct SyncEngine;

impl SyncEngine {
    /// Start the engine — one task per node plus a supervisor that owns
    /// the [`JoinSet`]. Additional nodes can join/leave later via
    /// [`SyncHandle::add_node`]/[`SyncHandle::remove_node`].
    pub fn spawn(
        projection: ProjectionStore,
        outbox: Outbox,
        nodes: impl IntoIterator<Item = NodeTarget>,
        options: SyncOptions,
    ) -> SyncHandle {
        let cancel = CancellationToken::new();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(64);
        let shared = Arc::new(Shared {
            projection,
            outbox,
            options,
            targets: Mutex::new(HashMap::new()),
            active_pumps: Arc::new(AtomicUsize::new(0)),
        });
        {
            let mut targets = shared
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for target in nodes {
                targets.insert(target.node.id.clone(), target);
            }
        }
        let supervisor = {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            tokio::spawn(async move { supervise(shared, cmd_rx, cancel).await })
        };
        SyncHandle {
            shared,
            cmds: cmd_tx,
            cancel,
            supervisor,
        }
    }
}

enum Cmd {
    Add(NodeTarget),
    Remove(String),
}

/// Handle to a running engine — cloneable; `shutdown` consumes it.
pub struct SyncHandle {
    shared: Arc<Shared>,
    cmds: mpsc::Sender<Cmd>,
    cancel: CancellationToken,
    supervisor: tokio::task::JoinHandle<()>,
}

impl SyncHandle {
    /// The projection the engine maintains — safe to read from anywhere.
    pub fn projection(&self) -> ProjectionStore {
        self.shared.projection.clone()
    }

    /// The outbox — inspect `dead_letters`/`pending_for_node` directly.
    pub fn outbox(&self) -> Outbox {
        self.shared.outbox.clone()
    }

    /// Live SSE pump threads — `0` means every node loop is idle or
    /// offline (used by tests to assert clean teardown).
    pub fn active_pumps(&self) -> usize {
        self.shared.active_pumps.load(Ordering::Relaxed)
    }

    /// The shared pump counter — cloneable so callers can keep
    /// observing teardown after `shutdown()` consumes the handle.
    pub fn pump_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.shared.active_pumps)
    }

    /// Register a node and start syncing it.
    ///
    /// # Errors
    /// [`SyncError::Shutdown`] if the engine is gone; sqlite failure
    /// writing the node row.
    pub async fn add_node(&self, target: NodeTarget) -> Result<(), SyncError> {
        self.shared.projection.upsert_node(&target.node)?;
        self.cmds
            .send(Cmd::Add(target))
            .await
            .map_err(|_| SyncError::Shutdown)
    }

    /// Stop syncing a node and drop its projected state.
    ///
    /// # Errors
    /// [`SyncError::Shutdown`] if the engine is gone; sqlite failure.
    pub async fn remove_node(&self, id: &str) -> Result<(), SyncError> {
        self.shared.projection.remove_node(id)?;
        self.cmds
            .send(Cmd::Remove(id.to_string()))
            .await
            .map_err(|_| SyncError::Shutdown)
    }

    /// A write to a node: posted immediately when the node is up;
    /// appended to the durable outbox (per-session FIFO) when it's down
    /// or the direct post fails on transport. HTTP rejections surface as
    /// errors — they were delivered and refused, not lost.
    ///
    /// # Errors
    /// [`SyncError::UnknownNode`] for unregistered ids,
    /// [`SyncError::Node`] when the node rejects the write, and
    /// outbox/sqlite failures.
    pub async fn submit_op(&self, write: NodeWrite) -> Result<SubmitOutcome, SyncError> {
        let target = {
            self.shared
                .targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&write.node_id)
                .cloned()
        };
        let Some(target) = target else {
            return Err(SyncError::UnknownNode(write.node_id.clone()));
        };

        let up = self
            .shared
            .projection
            .node(&write.node_id)?
            .is_some_and(|row| row.status == NodeStatus::Up);

        if up {
            let client = NodeClient::with_config(
                &target.node.url,
                target.token.clone(),
                self.client_config(),
            )?;
            let session_id = write.session_id.clone();
            let agent = write.agent.clone();
            let op = write.op.clone();
            let payload = write.payload.clone();
            let result = tokio::task::spawn_blocking(move || {
                client.post_op(&session_id, agent.as_deref(), &op, &payload)
            })
            .await
            .unwrap_or_else(|e| Err(NodeError::Task(e.to_string())));
            match result {
                Ok(value) => return Ok(SubmitOutcome::Applied(value)),
                Err(e) if e.is_transport() => {
                    // Fresh evidence the node is gone — reflect it and
                    // queue; the loop's reconnect drains in order.
                    let _ = self
                        .shared
                        .projection
                        .set_node_status(&write.node_id, NodeStatus::Down);
                }
                Err(e) => return Err(SyncError::Node(e)),
            }
        }

        let entry = self.shared.outbox.enqueue(
            &write.node_id,
            &write.session_id,
            &write.op,
            write.kind,
            write.payload,
            &write.idempotency_key,
            write.ttl_seconds,
        )?;
        Ok(SubmitOutcome::Queued(Box::new(entry)))
    }

    /// Stop every loop and await the supervisor. The SSE pump threads
    /// exit at the next feed frame or the `sse_max_age` read deadline —
    /// [`Self::active_pumps`] reports stragglers.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.supervisor.await;
    }

    fn client_config(&self) -> ClientConfig {
        self.shared.options.client_config()
    }
}

/// The supervisor — owns the `JoinSet` of per-node loops and the
/// add/remove command channel.
async fn supervise(shared: Arc<Shared>, mut rx: mpsc::Receiver<Cmd>, cancel: CancellationToken) {
    let mut set: JoinSet<()> = JoinSet::new();
    let mut tasks: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    {
        let targets: Vec<NodeTarget> = shared
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for target in targets {
            if shared.projection.upsert_node(&target.node).is_err() {
                tracing::warn!(node = %target.node.id, "failed to register node in projection");
            }
            let abort = set.spawn(node_loop(
                target.clone(),
                Arc::clone(&shared),
                cancel.clone(),
            ));
            tasks.insert(target.node.id.clone(), abort);
        }
    }
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Cmd::Add(target)) => {
                    shared
                        .targets
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(target.node.id.clone(), target.clone());
                    if tasks.contains_key(&target.node.id) {
                        continue;
                    }
                    let abort = set.spawn(node_loop(
                        target.clone(),
                        Arc::clone(&shared),
                        cancel.clone(),
                    ));
                    tasks.insert(target.node.id.clone(), abort);
                }
                Some(Cmd::Remove(id)) => {
                    shared
                        .targets
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&id);
                    if let Some(abort) = tasks.remove(&id) {
                        abort.abort();
                    }
                }
                None => break,
            },
            Some(Err(e)) = set.join_next() => {
                if e.is_panic() {
                    tracing::error!("node sync loop panicked: {e}");
                }
            }
            () = cancel.cancelled() => break,
        }
    }
    // `shutdown` aborts whatever didn't observe the token yet; the
    // loops' own `cancelled` arms exit promptly regardless.
    set.shutdown().await;
}

/* ---- per-node loop --------------------------------------------------*/

/// Why one connect-cycle ended.
enum CycleEnd {
    /// Engine shutdown.
    Cancelled,
    /// `list_sessions` failed — the node is down; exponential backoff.
    PreConnect(NodeError),
    /// We were connected and the SSE stream ended — quick reconnect.
    Disconnected(NodeError),
}

async fn node_loop(target: NodeTarget, shared: Arc<Shared>, cancel: CancellationToken) {
    let node_id = target.node.id.clone();
    let client = match NodeClient::with_config(
        &target.node.url,
        target.token.clone(),
        shared.options.client_config(),
    ) {
        Ok(client) => client,
        Err(e) => {
            tracing::error!(node = %node_id, "bad node config: {e}");
            return;
        }
    };
    let mut delay = shared.options.backoff_min;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match connect_once(&target.node, &client, &shared, &cancel).await {
            CycleEnd::Cancelled => return,
            CycleEnd::PreConnect(e) => {
                tracing::debug!(node = %node_id, "connect failed: {e}");
            }
            CycleEnd::Disconnected(e) => {
                tracing::info!(node = %node_id, "connection lost: {e}");
                delay = shared.options.backoff_min;
            }
        }
        if shared
            .projection
            .set_node_status(&node_id, NodeStatus::Down)
            .is_err()
        {
            tracing::warn!(node = %node_id, "failed to mark node down");
        }
        // A previously-healthy stream that dropped retries fast;
        // repeated connect failures back off exponentially.
        let wait = jittered(delay);
        delay = delay.saturating_mul(2).min(shared.options.backoff_max);
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = cancel.cancelled() => return,
        }
    }
}

/// One connect cycle: subscribe → list → reconcile → drain → apply.
/// Returns only when the subscription ends or the engine is cancelled.
///
/// Ordering: the SSE subscription opens *before* the listing so a diff
/// emitted while the list is in flight lands on the stream — anything
/// earlier is covered by the listing itself. The outbox drains last so
/// the feed events a replayed write triggers are observed.
async fn connect_once(
    node: &NodeRef,
    client: &NodeClient,
    shared: &Arc<Shared>,
    cancel: &CancellationToken,
) -> CycleEnd {
    // The pump is a dedicated blocking thread feeding an mpsc; the
    // async side stays cancel-instant.
    let (tx, mut rx) = mpsc::channel::<PumpMsg>(shared.options.feed_channel);
    let stop = Arc::new(AtomicBool::new(false));
    let pump = {
        let client = client.clone();
        let stop = Arc::clone(&stop);
        let shared = Arc::clone(shared);
        std::thread::spawn(move || sse_pump(&client, &tx, &stop, &shared))
    };

    // Wait for the subscription handshake (or connect failure).
    let subscribed = tokio::select! {
        () = cancel.cancelled() => {
            stop.store(true, Ordering::Relaxed);
            let _ = tokio::task::spawn_blocking(move || pump.join()).await;
            return CycleEnd::Cancelled;
        }
        msg = rx.recv() => match msg {
            Some(PumpMsg::Subscribed) => true,
            Some(PumpMsg::Closed(e)) => {
                return CycleEnd::PreConnect(e.unwrap_or_else(|| {
                    NodeError::Malformed("event stream closed before subscribe".into())
                }));
            }
            Some(PumpMsg::Event(_)) | None => {
                return CycleEnd::Disconnected(NodeError::Malformed(
                    "pump ended before subscribe".into(),
                ));
            }
        },
    };
    debug_assert!(subscribed);

    // Listing — a success means the node is reachable *and* the
    // projection is freshly reconciled (tombstones included).
    let summaries = match blocking(client.clone(), move |c| c.list_sessions(true)).await {
        Ok(s) => s,
        Err(e) => {
            stop.store(true, Ordering::Relaxed);
            let _ = tokio::task::spawn_blocking(move || pump.join()).await;
            return CycleEnd::Disconnected(e);
        }
    };
    if let Err(e) = shared.projection.apply_listing(&node.id, &summaries) {
        tracing::warn!(node = %node.id, "projection apply_listing failed: {e}");
    }
    if let Err(e) = shared.projection.set_node_status(&node.id, NodeStatus::Up) {
        tracing::warn!(node = %node.id, "failed to mark node up: {e}");
    }
    drain_outbox(node, client, shared).await;

    let mut resync = tokio::time::interval_at(
        tokio::time::Instant::now() + shared.options.resync_interval,
        shared.options.resync_interval,
    );
    let end = loop {
        tokio::select! {
            () = cancel.cancelled() => break CycleEnd::Cancelled,
            msg = rx.recv() => match msg {
                None => break CycleEnd::Disconnected(NodeError::Malformed("pump ended".into())),
                // The pump sends Subscribed exactly once, before any
                // frames — a duplicate is inert.
                Some(PumpMsg::Subscribed) => {}
                Some(PumpMsg::Closed(e)) => {
                    break CycleEnd::Disconnected(e.unwrap_or_else(|| {
                        NodeError::Malformed("event stream ended".into())
                    }));
                }
                Some(PumpMsg::Event(event)) => {
                    if let Err(e) = apply_event(node, client, shared, event).await {
                        tracing::warn!(node = %node.id, "apply event failed: {e}");
                    }
                }
            },
            _ = resync.tick() => {
                match blocking(client.clone(), |c| c.list_sessions(true)).await {
                    Ok(summaries) => {
                        if let Err(e) = shared.projection.apply_listing(&node.id, &summaries) {
                            tracing::warn!(node = %node.id, "resync failed: {e}");
                        }
                    }
                    Err(e) => break CycleEnd::Disconnected(e),
                }
            }
        }
    };

    stop.store(true, Ordering::Relaxed);
    // Bounded by `sse_max_age` (the read deadline) — keeps the loop's
    // task slot clean while the pump wakes on the next heartbeat.
    let _ = tokio::task::spawn_blocking(move || pump.join()).await;
    end
}

/// Blocking SSE pump — one std thread per live connection. Reads
/// frames, forwards them to the async loop, checks `stop` between
/// frames (the node's heartbeat cadence and the `sse_max_age` body
/// deadline bound the wakeup latency).
fn sse_pump(
    client: &NodeClient,
    tx: &mpsc::Sender<PumpMsg>,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
) {
    shared.active_pumps.fetch_add(1, Ordering::Relaxed);
    let finish = |msg: PumpMsg| {
        let _ = tx.blocking_send(msg);
        shared.active_pumps.fetch_sub(1, Ordering::Relaxed);
    };
    match client.events() {
        Err(e) => finish(PumpMsg::Closed(Some(e))),
        Ok(mut stream) => {
            // Acknowledge the subscription before frames — the async
            // side lists only after this, closing the diff gap.
            if tx.blocking_send(PumpMsg::Subscribed).is_err() {
                shared.active_pumps.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            loop {
                if stop.load(Ordering::Relaxed) {
                    shared.active_pumps.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
                match stream.next_event() {
                    Ok(Some(event)) => {
                        if tx.blocking_send(PumpMsg::Event(event)).is_err() {
                            shared.active_pumps.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                    }
                    Ok(None) => {
                        finish(PumpMsg::Closed(None));
                        return;
                    }
                    Err(e) => {
                        finish(PumpMsg::Closed(Some(e)));
                        return;
                    }
                }
            }
        }
    }
}

/// Drain queued writes oldest-first. Per-session FIFO is preserved by
/// skipping the rest of a session's entries once one fails back to
/// `pending` (it stays head-of-queue for the next drain).
async fn drain_outbox(node: &NodeRef, client: &NodeClient, shared: &Arc<Shared>) {
    let pending = match shared.outbox.pending_for_node(&node.id) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(node = %node.id, "outbox read failed: {e}");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    tracing::info!(node = %node.id, "draining {} queued write(s)", pending.len());
    let mut blocked: HashSet<String> = HashSet::new();
    for entry in pending {
        if blocked.contains(&entry.session_id) {
            continue;
        }
        if let Err(e) = shared.outbox.mark_in_flight(&entry.id) {
            tracing::warn!(node = %node.id, "mark_in_flight failed: {e}");
            continue;
        }
        // `?agent` scope comes from the projection — the outbox only
        // stores the session id.
        let agent = shared
            .projection
            .session(&node.id, &entry.session_id)
            .ok()
            .flatten()
            .map(|s| s.agent)
            .filter(|a| !a.is_empty());
        let session_id = entry.session_id.clone();
        let op = entry.op.clone();
        let payload = entry.payload.clone();
        let result = blocking(client.clone(), move |c| {
            c.post_op(&session_id, agent.as_deref(), &op, &payload)
        })
        .await;
        match result {
            Ok(_) => {
                if let Err(e) = shared.outbox.mark_done(&entry.id) {
                    tracing::warn!(node = %node.id, "mark_done failed: {e}");
                }
            }
            Err(e) => {
                tracing::warn!(node = %node.id, op = %entry.op, "replay failed: {e}");
                match shared.outbox.mark_failed(&entry.id, &e.to_string()) {
                    Ok(OutboxStatus::Pending) => {
                        blocked.insert(entry.session_id.clone());
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!(node = %node.id, "mark_failed failed: {err}"),
                }
            }
        }
    }
}

/// Apply one feed frame to the projection.
async fn apply_event(
    node: &NodeRef,
    client: &NodeClient,
    shared: &Arc<Shared>,
    event: FeedEvent,
) -> Result<(), SyncError> {
    match event {
        FeedEvent::Diff {
            kind, id, patch, ..
        } if kind == "session" => {
            if patch.get("deleted") == Some(&Value::Bool(true)) {
                shared.projection.delete_session(&node.id, &id)?;
                return Ok(());
            }
            patch_or_refetch(node, client, shared, &id, &patch).await
        }
        FeedEvent::Diff {
            kind, id, patch, ..
        } if kind == "meta" => {
            // `{deleted: true}` on meta means the overlay row was
            // removed — the session itself survives; refetch.
            if patch.get("deleted") == Some(&Value::Bool(true)) {
                refetch(node, client, shared, &id).await
            } else {
                patch_or_refetch(node, client, shared, &id, &patch).await
            }
        }
        FeedEvent::Lagged => {
            let summaries = blocking(client.clone(), |c| c.list_sessions(true)).await?;
            shared.projection.apply_listing(&node.id, &summaries)?;
            Ok(())
        }
        // `project` diffs don't change session rows; heartbeats,
        // session-stream frames, and unknown frames are inert here.
        _ => Ok(()),
    }
}

/// Merge a diff, fetching the full summary when the row isn't indexed
/// yet (a `meta` patch can precede the session's first list).
async fn patch_or_refetch(
    node: &NodeRef,
    client: &NodeClient,
    shared: &Arc<Shared>,
    id: &str,
    patch: &Value,
) -> Result<(), SyncError> {
    match shared.projection.apply_patch(&node.id, id, patch)? {
        PatchStatus::Applied => Ok(()),
        PatchStatus::Missing => refetch(node, client, shared, id).await,
    }
}

async fn refetch(
    node: &NodeRef,
    client: &NodeClient,
    shared: &Arc<Shared>,
    id: &str,
) -> Result<(), SyncError> {
    let session_id = id.to_string();
    let result = blocking(client.clone(), move |c| c.get_summary(&session_id, None)).await;
    match result {
        Ok(summary) => {
            shared.projection.upsert_summary(&node.id, &summary)?;
        }
        Err(NodeError::Http { status: 404, .. }) => {
            shared.projection.delete_session(&node.id, id)?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// `spawn_blocking` wrapper — `JoinError` becomes [`NodeError::Task`].
async fn blocking<T, F>(client: NodeClient, f: F) -> Result<T, NodeError>
where
    T: Send + 'static,
    F: FnOnce(&NodeClient) -> Result<T, NodeError> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || f(&client)).await {
        Ok(result) => result,
        Err(e) => Err(NodeError::Task(e.to_string())),
    }
}

/// Capped exponential backoff with ±25% jitter — `RandomState` provides
/// the entropy so no `rand` dep is needed.
fn jittered(base: Duration) -> Duration {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    let pct = 75 + u32::try_from(hasher.finish() % 51).unwrap_or(25);
    base.mul_f64(f64::from(pct) / 100.0)
}
