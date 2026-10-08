//! sepia-sync — the hub-side sync layer.
//!
//! Keeps a local SQLite projection ([`ProjectionStore`]) of every paired
//! node's session index: summary rows only — titles, locks, busy flags —
//! never message bodies (those stay node-fetched via
//! [`client::NodeClient::get_history`]). The [`SyncEngine`] runs one
//! supervised loop per node: connect → `list_sessions` → drain the
//! durable [`Outbox`] → tail `/api/events` for summary diffs →
//! reconnect with capped exponential backoff + jitter.
//!
//! Writes while a node is down go through [`SyncHandle::submit_op`],
//! which posts directly when the node is up and enqueues into
//! `sepia-outbox` otherwise.

pub mod client;
pub mod engine;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use client::{ClientConfig, EventStream, FeedEvent, HistoryPage, NodeClient, NodeError};
pub use engine::{NodeTarget, SubmitOutcome, SyncEngine, SyncHandle, SyncOptions};
pub use sepia_outbox::{OpKind, Outbox, OutboxEntry, Status as OutboxStatus};

/// Failures of the sync layer — store, outbox, and node calls share one
/// error type at the engine boundary.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Projection-store failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Outbox failure.
    #[error("outbox: {0}")]
    Outbox(#[from] sepia_outbox::OutboxError),
    /// A node call failed (transport, HTTP status, malformed body).
    #[error("node: {0}")]
    Node(#[from] NodeError),
    /// JSON (de)serialization failure.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// The node id isn't registered with the engine.
    #[error("unknown node {0}")]
    UnknownNode(String),
    /// The engine is already shut down (command channel closed).
    #[error("sync engine is shut down")]
    Shutdown,
}

/// A registered peer node — the identity the hub syncs and writes to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRef {
    /// Stable node id (`/api/node`'s `id`, or a hub-assigned label-safe id).
    pub id: String,
    /// Base URL, e.g. `http://192.168.1.10:8787` (no trailing slash needed).
    pub url: String,
    /// Display name in the hub UI.
    pub label: String,
}

/// Last known reachability of a node — written to `nodes.status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeStatus {
    /// Never contacted since registration.
    #[default]
    Unknown,
    /// A request (list/events) succeeded recently.
    Up,
    /// The last connection attempt failed or the SSE stream dropped.
    Down,
}

impl NodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "up" => Self::Up,
            "down" => Self::Down,
            _ => Self::Unknown,
        }
    }
}

/// A `nodes` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRow {
    pub id: String,
    pub url: String,
    pub label: String,
    /// RFC 3339 — last time a request to the node succeeded.
    pub last_seen_at: Option<String>,
    pub status: NodeStatus,
}

/// A `session_index` row — one node's session summary. `raw` keeps the
/// whole wire object so fields the projection doesn't columnize
/// (`pinned`, `projectIds`, `spans`, …) still reach the hub UI.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedSession {
    pub node_id: String,
    pub session_id: String,
    pub title: String,
    pub cwd: String,
    /// The agent store backend (`source` on the wire).
    pub backend: String,
    /// Owning agent id (`agent` on the wire).
    pub agent: String,
    /// RFC 3339 `updatedAt` from the node.
    pub updated_at: String,
    pub locked: bool,
    pub busy: bool,
    /// `lastActivityAt` — same instant as `updated_at` on today's wire.
    pub last_activity_at: String,
    /// The full summary `Value` the node sent.
    pub raw: Value,
}

/// What [`ProjectionStore::apply_patch`] did with a summary diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchStatus {
    /// Merged into an existing row.
    Applied,
    /// No row for `(node, session)` — the caller should refetch the
    /// full summary.
    Missing,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS nodes (
    id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    label TEXT NOT NULL,
    last_seen_at TEXT,
    status TEXT NOT NULL DEFAULT 'unknown'
);
CREATE TABLE IF NOT EXISTS session_index (
    node_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT '',
    backend TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT '',
    locked INTEGER NOT NULL DEFAULT 0,
    busy INTEGER NOT NULL DEFAULT 0,
    last_activity_at TEXT NOT NULL DEFAULT '',
    raw_json TEXT NOT NULL,
    PRIMARY KEY (node_id, session_id)
);
CREATE INDEX IF NOT EXISTS session_index_node ON session_index (node_id);
CREATE TABLE IF NOT EXISTS sync_state (
    node_id TEXT PRIMARY KEY,
    cursor TEXT,
    updated_at TEXT NOT NULL
);
";

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Pull the column values out of one wire summary — unknown/missing
/// fields degrade to defaults rather than failing the row.
fn columns_of(raw: &Value) -> IndexedSession {
    let get = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or_default();
    let flag = |k: &str| raw.get(k).and_then(Value::as_bool).unwrap_or(false);
    let updated_at = get("updatedAt").to_string();
    IndexedSession {
        node_id: String::new(),
        session_id: get("id").to_string(),
        title: get("title").to_string(),
        cwd: get("cwd").to_string(),
        backend: get("source").to_string(),
        agent: get("agent").to_string(),
        last_activity_at: raw
            .get("lastActivityAt")
            .and_then(Value::as_str)
            .unwrap_or(&updated_at)
            .to_string(),
        updated_at,
        locked: flag("locked"),
        busy: flag("busy"),
        raw: raw.clone(),
    }
}

fn write_row(conn: &Connection, node_id: &str, summary: &Value) -> Result<(), SyncError> {
    let mut row = columns_of(summary);
    row.node_id = node_id.to_string();
    conn.execute(
        "INSERT INTO session_index
         (node_id, session_id, title, cwd, backend, agent, updated_at, locked, busy, last_activity_at, raw_json)
         VALUES (?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT (node_id, session_id) DO UPDATE SET
            title=excluded.title, cwd=excluded.cwd, backend=excluded.backend,
            agent=excluded.agent, updated_at=excluded.updated_at, locked=excluded.locked,
            busy=excluded.busy, last_activity_at=excluded.last_activity_at,
            raw_json=excluded.raw_json",
        params![
            row.node_id,
            row.session_id,
            row.title,
            row.cwd,
            row.backend,
            row.agent,
            row.updated_at,
            row.locked,
            row.busy,
            row.last_activity_at,
            serde_json::to_string(&row.raw)?,
        ],
    )?;
    Ok(())
}

fn row_to_indexed(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedSession> {
    let raw_json: String = row.get("raw_json")?;
    let raw = serde_json::from_str(&raw_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(IndexedSession {
        node_id: row.get("node_id")?,
        session_id: row.get("session_id")?,
        title: row.get("title")?,
        cwd: row.get("cwd")?,
        backend: row.get("backend")?,
        agent: row.get("agent")?,
        updated_at: row.get("updated_at")?,
        locked: row.get::<_, i64>("locked")? != 0,
        busy: row.get::<_, i64>("busy")? != 0,
        last_activity_at: row.get("last_activity_at")?,
        raw,
    })
}

/// The local projection — a materialized view of every paired node's
/// session index plus per-node liveness and resync cursors. `Clone`
/// shares one connection (same pattern as `sepia-outbox`).
#[derive(Clone)]
pub struct ProjectionStore {
    conn: Arc<Mutex<Connection>>,
}

impl ProjectionStore {
    /// Open (and create) a projection at `path` — WAL + busy_timeout,
    /// matching `sepia-outbox`'s durability posture.
    ///
    /// # Errors
    /// On sqlite open/schema failure.
    pub fn open(path: &Path) -> Result<Self, SyncError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory projection for tests.
    ///
    /// # Errors
    /// On schema failure.
    pub fn in_memory() -> Result<Self, SyncError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /* ---- nodes ------------------------------------------------------*/

    /// Register (or refresh) a node row. Status is untouched on update
    /// so a re-register doesn't flap liveness.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn upsert_node(&self, node: &NodeRef) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute(
            "INSERT INTO nodes (id, url, label, status) VALUES (?,?,?,'unknown')
             ON CONFLICT (id) DO UPDATE SET url=excluded.url, label=excluded.label",
            params![node.id, node.url, node.label],
        )?;
        Ok(())
    }

    /// Drop a node and all of its projected state.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn remove_node(&self, id: &str) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute("DELETE FROM session_index WHERE node_id=?", params![id])?;
        conn.execute("DELETE FROM sync_state WHERE node_id=?", params![id])?;
        conn.execute("DELETE FROM nodes WHERE id=?", params![id])?;
        Ok(())
    }

    /// # Errors
    /// On sqlite failure.
    pub fn node(&self, id: &str) -> Result<Option<NodeRow>, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stmt =
            conn.prepare("SELECT id, url, label, last_seen_at, status FROM nodes WHERE id=?")?;
        let mut rows = stmt.query_map(params![id], node_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every registered node row.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn nodes(&self) -> Result<Vec<NodeRow>, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stmt =
            conn.prepare("SELECT id, url, label, last_seen_at, status FROM nodes ORDER BY id")?;
        let rows = stmt
            .query_map([], node_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Record reachability. `Up` also stamps `last_seen_at`.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn set_node_status(&self, id: &str, status: NodeStatus) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status == NodeStatus::Up {
            conn.execute(
                "UPDATE nodes SET status='up', last_seen_at=? WHERE id=?",
                params![now_rfc3339(), id],
            )?;
        } else {
            conn.execute(
                "UPDATE nodes SET status=? WHERE id=?",
                params![status.as_str(), id],
            )?;
        }
        Ok(())
    }

    /* ---- session_index ----------------------------------------------*/

    /// Upsert one wire summary row under `node_id`.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn upsert_summary(&self, node_id: &str, summary: &Value) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        write_row(&conn, node_id, summary)
    }

    /// Replace a node's whole index: upsert every listed summary, delete
    /// rows the list no longer reports (tombstones), and advance the
    /// `sync_state` cursor to the max `updatedAt` seen.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn apply_listing(&self, node_id: &str, summaries: &[Value]) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = conn.unchecked_transaction()?;
        let mut keep = Vec::with_capacity(summaries.len());
        let mut max_updated = String::new();
        for summary in summaries {
            write_row(&tx, node_id, summary)?;
            if let Some(id) = summary.get("id").and_then(Value::as_str) {
                keep.push(id.to_string());
            }
            if let Some(u) = summary.get("updatedAt").and_then(Value::as_str) {
                if u > max_updated.as_str() {
                    max_updated = u.to_string();
                }
            }
        }
        if keep.is_empty() {
            tx.execute(
                "DELETE FROM session_index WHERE node_id=?",
                params![node_id],
            )?;
        } else {
            // `IN` list — ids come from the node, not the user, but
            // still go through bind params.
            let marks = std::iter::repeat_n("?", keep.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM session_index WHERE node_id=? AND session_id NOT IN ({marks})"
            );
            let mut bind: Vec<&dyn rusqlite::ToSql> = vec![&node_id];
            bind.extend(keep.iter().map(|id| id as &dyn rusqlite::ToSql));
            tx.execute(&sql, rusqlite::params_from_iter(bind.iter()))?;
        }
        tx.execute(
            "INSERT INTO sync_state (node_id, cursor, updated_at) VALUES (?,?,?)
             ON CONFLICT (node_id) DO UPDATE SET cursor=excluded.cursor, updated_at=excluded.updated_at",
            params![node_id, max_updated, now_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Merge a `{patch}` diff into an indexed row's `raw_json` and
    /// re-derive its columns. [`PatchStatus::Missing`] when no row
    /// exists — the caller should fetch the full summary.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn apply_patch(
        &self,
        node_id: &str,
        session_id: &str,
        patch: &Value,
    ) -> Result<PatchStatus, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let raw_json: Option<String> = conn
            .query_row(
                "SELECT raw_json FROM session_index WHERE node_id=? AND session_id=?",
                params![node_id, session_id],
                |r| r.get(0),
            )
            .ok();
        let Some(raw_json) = raw_json else {
            return Ok(PatchStatus::Missing);
        };
        let mut raw: Value = serde_json::from_str(&raw_json)?;
        if let (Some(obj), Some(patch_obj)) = (raw.as_object_mut(), patch.as_object()) {
            for (k, v) in patch_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
        write_row(&conn, node_id, &raw)?;
        Ok(PatchStatus::Applied)
    }

    /// Drop one projected session (tombstone applied).
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn delete_session(&self, node_id: &str, session_id: &str) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute(
            "DELETE FROM session_index WHERE node_id=? AND session_id=?",
            params![node_id, session_id],
        )?;
        Ok(())
    }

    /// One projected session row.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn session(
        &self,
        node_id: &str,
        session_id: &str,
    ) -> Result<Option<IndexedSession>, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stmt =
            conn.prepare("SELECT * FROM session_index WHERE node_id=? AND session_id=?")?;
        let mut rows = stmt.query_map(params![node_id, session_id], row_to_indexed)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every projected session — `node_id` scopes to one node, `None`
    /// lists all nodes (the hub's merged session list).
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn sessions(&self, node_id: Option<&str>) -> Result<Vec<IndexedSession>, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(node_id) = node_id {
            let mut stmt = conn
                .prepare("SELECT * FROM session_index WHERE node_id=? ORDER BY updated_at DESC")?;
            let rows = stmt
                .query_map(params![node_id], row_to_indexed)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        } else {
            let mut stmt = conn.prepare("SELECT * FROM session_index ORDER BY updated_at DESC")?;
            let rows = stmt
                .query_map([], row_to_indexed)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }

    /// The resync cursor for a node (max `updatedAt` from the last full
    /// listing — informational; today's resync is always a full list).
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn cursor(&self, node_id: &str) -> Result<Option<String>, SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cursor = conn
            .query_row(
                "SELECT cursor FROM sync_state WHERE node_id=?",
                params![node_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        Ok(cursor)
    }

    /// Record a resync cursor explicitly.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn set_cursor(&self, node_id: &str, cursor: &str) -> Result<(), SyncError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute(
            "INSERT INTO sync_state (node_id, cursor, updated_at) VALUES (?,?,?)
             ON CONFLICT (node_id) DO UPDATE SET cursor=excluded.cursor, updated_at=excluded.updated_at",
            params![node_id, cursor, now_rfc3339()],
        )?;
        Ok(())
    }
}

fn node_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        id: row.get("id")?,
        url: row.get("url")?,
        label: row.get("label")?,
        last_seen_at: row.get("last_seen_at")?,
        status: NodeStatus::parse(&row.get::<_, String>("status")?),
    })
}

/// A write destined for a node — the input to [`SyncHandle::submit_op`].
/// When the node is down it becomes an [`OutboxEntry`]; `kind` decides
/// retry-vs-dead-letter on replay failure.
#[derive(Clone, Debug)]
pub struct NodeWrite {
    /// Target node (must be registered).
    pub node_id: String,
    /// Session the write applies to.
    pub session_id: String,
    /// Agent hint for `?agent=` — the projection's `agent` column is a
    /// good source.
    pub agent: Option<String>,
    /// Outbox op name: `prompt`, `cancel`, `permission`, `meta.patch`,
    /// `delete`.
    pub op: String,
    /// [`OpKind::Metadata`] retries; [`OpKind::Turn`] dead-letters on a
    /// failed replay.
    pub kind: OpKind,
    /// The JSON body (empty object for `cancel`/`delete`).
    pub payload: Value,
    /// Client-supplied dedupe key — re-submitting the same key returns
    /// the same queued entry instead of a duplicate write.
    pub idempotency_key: String,
    /// Seconds until the queued write expires (dead-letters); `None`
    /// never expires.
    pub ttl_seconds: Option<i64>,
}
