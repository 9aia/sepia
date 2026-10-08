//! sepia-outbox — durable queued writes for offline nodes.
//!
//! When a target node is unreachable the hub appends the intended write
//! (prompt, cancel, permission response, meta patch…) here instead of
//! dropping it. Entries drain in per-session FIFO order once the node is
//! reachable again; entries that exhaust their retry budget or TTL move
//! to dead-letter rather than disappearing.
//!
//! Ordering guarantee: per `(node_id, session_id)` the queue is FIFO.
//! Metadata ops (rename, pin) are safe under last-writer-wins replay;
//! turn-shaped ops (prompt/cancel/permission) dead-letter on failure
//! rather than interleave with newer writes — the caller decides.

use std::path::Path;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use time::OffsetDateTime;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS outbox (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    op TEXT NOT NULL,
    payload TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    last_error TEXT,
    seq INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS outbox_pending
    ON outbox (node_id, session_id, seq) WHERE status = 'pending';
";

#[derive(Debug, Error)]
pub enum OutboxError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("duplicate idempotency key {0}")]
    Duplicate(String),
}

/// The kind of write — drives conflict policy on replay failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpKind {
    /// Metadata-shaped (rename, pin, project assignment): safe under
    /// last-writer-wins, retry freely.
    Metadata,
    /// Turn-shaped (prompt, cancel, permission response): on failure
    /// dead-letter instead of reordering into a live conversation.
    Turn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pending,
    InFlight,
    Done,
    Dead,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Done => "done",
            Self::Dead => "dead",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "in_flight" => Self::InFlight,
            "done" => Self::Done,
            "dead" => Self::Dead,
            _ => Self::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEntry {
    pub id: String,
    pub node_id: String,
    pub session_id: String,
    /// Free-form op name (`prompt`, `cancel`, `meta.patch`, …).
    pub op: String,
    pub kind: OpKind,
    pub payload: Value,
    pub idempotency_key: String,
    pub status: Status,
    pub attempts: i64,
    pub max_attempts: i64,
    pub created_at: String,
    /// RFC3339; `None` never expires.
    pub expires_at: Option<String>,
    pub last_error: Option<String>,
    /// Monotonic sequence — the per-session FIFO order.
    pub seq: i64,
}

/// Durable outbox backed by a sqlite file. `Clone` shares one connection.
#[derive(Clone)]
pub struct Outbox {
    conn: std::sync::Arc<std::sync::Mutex<Connection>>,
}

impl Outbox {
    /// # Errors
    /// On sqlite open/schema failure.
    pub fn open(path: &Path) -> Result<Self, OutboxError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: std::sync::Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// In-memory outbox for tests.
    ///
    /// # Errors
    /// On schema failure.
    pub fn in_memory() -> Result<Self, OutboxError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: std::sync::Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// Enqueue a write. Re-enqueueing the same `idempotency_key` returns
    /// the existing entry instead of duplicating the write.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue(
        &self,
        node_id: &str,
        session_id: &str,
        op: &str,
        kind: OpKind,
        payload: Value,
        idempotency_key: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<OutboxEntry, OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = Self::by_key_locked(&conn, idempotency_key)? {
            return Ok(existing);
        }
        let now = OffsetDateTime::now_utc();
        let expires_at = ttl_seconds.map(|ttl| {
            (now + time::Duration::seconds(ttl))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });
        let entry = OutboxEntry {
            id: uuid::Uuid::new_v4().to_string(),
            node_id: node_id.into(),
            session_id: session_id.into(),
            op: op.into(),
            kind,
            payload,
            idempotency_key: idempotency_key.into(),
            status: Status::Pending,
            attempts: 0,
            max_attempts: 5,
            created_at: now
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            expires_at,
            last_error: None,
            seq: next_seq_locked(&conn, node_id, session_id)?,
        };
        conn.execute(
            "INSERT INTO outbox (id,node_id,session_id,op,payload,idempotency_key,status,attempts,max_attempts,created_at,expires_at,seq)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                entry.id,
                entry.node_id,
                entry.session_id,
                entry.op,
                serde_json::to_string(&Envelope {
                    kind: entry.kind,
                    payload: &entry.payload
                })?,
                entry.idempotency_key,
                entry.status.as_str(),
                entry.attempts,
                entry.max_attempts,
                entry.created_at,
                entry.expires_at,
                entry.seq,
            ],
        )?;
        Ok(entry)
    }

    /// The next pending entry for a `(node, session)` — the head of its
    /// FIFO. Expired entries are dead-lettered as a side effect.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn peek(
        &self,
        node_id: &str,
        session_id: &str,
    ) -> Result<Option<OutboxEntry>, OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::expire_locked(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT * FROM outbox WHERE node_id=? AND session_id=? AND status='pending'
             ORDER BY seq LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![node_id, session_id], row_to_entry)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Mark an entry in-flight (a replay attempt started).
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn mark_in_flight(&self, id: &str) -> Result<(), OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute(
            "UPDATE outbox SET status='in_flight', attempts=attempts+1 WHERE id=?",
            params![id],
        )?;
        Ok(())
    }

    /// Replay succeeded — remove the entry.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn mark_done(&self, id: &str) -> Result<(), OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute("UPDATE outbox SET status='done' WHERE id=?", params![id])?;
        Ok(())
    }

    /// Replay failed. `Turn` ops dead-letter immediately (do not reorder
    /// turn traffic); `Metadata` ops retry until `max_attempts`.
    ///
    /// # Errors
    /// On sqlite failure.
    pub fn mark_failed(&self, id: &str, error: &str) -> Result<Status, OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = Self::by_id_locked(&conn, id)?
            .ok_or_else(|| OutboxError::Duplicate(format!("unknown entry {id}")))?;
        let status = if entry.kind == OpKind::Turn || entry.attempts >= entry.max_attempts {
            Status::Dead
        } else {
            Status::Pending
        };
        conn.execute(
            "UPDATE outbox SET status=?, last_error=? WHERE id=?",
            params![status.as_str(), error, id],
        )?;
        Ok(status)
    }

    /// Pending/dead entries for a node, FIFO per session.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn pending_for_node(&self, node_id: &str) -> Result<Vec<OutboxEntry>, OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::expire_locked(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT * FROM outbox WHERE node_id=? AND status='pending' ORDER BY session_id, seq",
        )?;
        let rows = stmt
            .query_map(params![node_id], row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Dead-lettered entries — surfaced in the UI as `failed` writes.
    ///
    /// # Errors
    /// On sqlite/serde failure.
    pub fn dead_letters(&self) -> Result<Vec<OutboxEntry>, OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stmt = conn.prepare("SELECT * FROM outbox WHERE status='dead' ORDER BY seq")?;
        let rows = stmt
            .query_map([], row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Resurrect a dead-lettered entry for another attempt.
    ///
    /// # Errors
    /// On sqlite failure or unknown id.
    pub fn retry_dead(&self, id: &str) -> Result<(), OutboxError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.execute(
            "UPDATE outbox SET status='pending', attempts=0, last_error=NULL WHERE id=? AND status='dead'",
            params![id],
        )?;
        Ok(())
    }

    fn by_key_locked(conn: &Connection, key: &str) -> Result<Option<OutboxEntry>, OutboxError> {
        let mut stmt = conn.prepare("SELECT * FROM outbox WHERE idempotency_key=?")?;
        let mut rows = stmt.query_map(params![key], row_to_entry)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    fn by_id_locked(conn: &Connection, id: &str) -> Result<Option<OutboxEntry>, OutboxError> {
        let mut stmt = conn.prepare("SELECT * FROM outbox WHERE id=?")?;
        let mut rows = stmt.query_map(params![id], row_to_entry)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Move expired pending entries to dead-letter.
    fn expire_locked(conn: &Connection) -> Result<(), OutboxError> {
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        conn.execute(
            "UPDATE outbox SET status='dead', last_error='expired'
             WHERE status='pending' AND expires_at IS NOT NULL AND expires_at < ?",
            params![now],
        )?;
        Ok(())
    }
}

/// `kind` rides inside the payload column so the schema stays one table.
#[derive(Serialize)]
struct Envelope<'a> {
    kind: OpKind,
    payload: &'a Value,
}

fn next_seq_locked(conn: &Connection, node_id: &str, session_id: &str) -> Result<i64, OutboxError> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM outbox WHERE node_id=? AND session_id=?",
        params![node_id, session_id],
        |r| r.get(0),
    )?;
    Ok(seq)
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxEntry> {
    let payload_json: String = row.get("payload")?;
    let envelope: EnvelopeOwned = serde_json::from_str(&payload_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(OutboxEntry {
        id: row.get("id")?,
        node_id: row.get("node_id")?,
        session_id: row.get("session_id")?,
        op: row.get("op")?,
        kind: envelope.kind,
        payload: envelope.payload,
        idempotency_key: row.get("idempotency_key")?,
        status: Status::parse(&row.get::<_, String>("status")?),
        attempts: row.get("attempts")?,
        max_attempts: row.get("max_attempts")?,
        created_at: row.get("created_at")?,
        expires_at: row.get("expires_at")?,
        last_error: row.get("last_error")?,
        seq: row.get("seq")?,
    })
}

#[derive(Deserialize)]
struct EnvelopeOwned {
    kind: OpKind,
    payload: Value,
}
