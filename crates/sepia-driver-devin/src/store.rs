//! `sessions.db` SessionRepository — the SqliteStorage port. Read paths
//! never migrate or WAL-flip a live store; writes run inside a
//! transaction. Read caches mirror the TS tiering: a mtime-keyed list
//! cache, a stamp-keyed session cache, and a TTL-graced calls index.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rusqlite::{Connection, params};
use sepia_core::domain::{MessageNode, PromptHistoryEntry, Session, StorageError, ToolCallStatus};
use sepia_core::shared::{self, ToolCallOutcome};
use sepia_core::storage::{
    NodesWindowOptions, SessionNodeWindow, SessionRepository, needs_migration,
};
use serde_json::Value;

use crate::mapping;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
  id text PRIMARY KEY NOT NULL,
  working_directory text NOT NULL,
  backend_type text NOT NULL,
  model text NOT NULL,
  agent_mode text NOT NULL,
  created_at integer NOT NULL,
  last_activity_at integer NOT NULL,
  title text NOT NULL,
  main_chain_id integer NOT NULL,
  shell_last_seen_index integer DEFAULT 0 NOT NULL,
  cogs_json text DEFAULT '[]' NOT NULL,
  workspace_dirs text DEFAULT '[]' NOT NULL,
  hidden integer DEFAULT 0 NOT NULL,
  metadata text DEFAULT '{}' NOT NULL
);
CREATE TABLE IF NOT EXISTS message_nodes (
  row_id integer PRIMARY KEY AUTOINCREMENT NOT NULL,
  session_id text NOT NULL,
  node_id integer NOT NULL,
  parent_node_id integer,
  chat_message text NOT NULL,
  created_at integer NOT NULL,
  metadata text,
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON UPDATE no action ON DELETE no action
);
CREATE TABLE IF NOT EXISTS prompt_history (
  id integer PRIMARY KEY AUTOINCREMENT NOT NULL,
  content text NOT NULL,
  timestamp integer NOT NULL,
  session_id text NOT NULL,
  is_shell integer DEFAULT 0 NOT NULL,
  FOREIGN KEY (session_id) REFERENCES sessions(id) ON UPDATE no action ON DELETE no action
);
";

const SESSION_CACHE_MAX: usize = 64;
const SESSION_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
const CALLS_INDEX_TTL: Duration = Duration::from_secs(15);

struct SessionStamp {
    last_activity_at: f64,
    backend_type: String,
    node_count: i64,
    prompt_count: i64,
    tool_state_count: i64,
    max_node_id: i64,
}

impl SessionStamp {
    fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.last_activity_at, self.node_count, self.prompt_count, self.tool_state_count
        )
    }
}

type NodeRow = (i64, Option<i64>, String, f64, Option<String>);

struct CallsEntry {
    max_node_id: i64,
    built_at: Instant,
    nodes: Vec<MessageNode>,
}

/// A `sessions.db` opened once; read-only mode never writes, migrates, or
/// switches journal mode.
pub struct DevinStore {
    db_path: PathBuf,
    conn: std::sync::Mutex<Connection>,
    readonly: bool,
    has_tool_call_state: bool,
    has_subagent_heads: bool,
    list_cache: std::sync::Mutex<Option<(f64, Vec<Session>)>>,
    session_cache: std::sync::Mutex<HashMap<String, (String, Session)>>,
    calls_index: std::sync::Mutex<HashMap<String, CallsEntry>>,
}

fn err(prefix: &str) -> impl Fn(rusqlite::Error) -> StorageError + '_ {
    move |e| StorageError::new(format!("{prefix}: {e}"))
}

impl DevinStore {
    /// Open the store. `readonly` skips WAL + migration — the mode a live
    /// Devin install demands.
    ///
    /// # Errors
    /// `StorageError` on sqlite open/schema failure.
    pub fn open(db_path: &Path, readonly: bool) -> Result<Self, StorageError> {
        let conn = if readonly {
            Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
        } else {
            Connection::open(db_path)
        }
        .map_err(err("Failed to open database"))?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(err("Failed to open database"))?;
        if !readonly && db_path != Path::new(":memory:") {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(err("Failed to open database"))?;
        }
        let tables = table_names(&conn)?;
        if !readonly && needs_migration(&tables) {
            conn.execute_batch(SCHEMA)
                .map_err(err("Failed to open database"))?;
        }
        let has_tool_call_state = tables.contains("tool_call_state");
        let has_subagent_heads = tables.contains("subagent_heads");
        Ok(Self {
            db_path: db_path.to_path_buf(),
            conn: std::sync::Mutex::new(conn),
            readonly,
            has_tool_call_state,
            has_subagent_heads,
            list_cache: std::sync::Mutex::new(None),
            session_cache: std::sync::Mutex::new(HashMap::new()),
            calls_index: std::sync::Mutex::new(HashMap::new()),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn has_tool_call_state(&self) -> bool {
        self.has_tool_call_state
    }

    pub fn has_subagent_heads(&self) -> bool {
        self.has_subagent_heads
    }

    fn db_stamp(&self) -> f64 {
        if self.db_path == Path::new(":memory:") {
            return 0.0;
        }
        let mut stamp = 0.0f64;
        for suffix in ["", "-wal"] {
            let p = self.db_path.with_file_name(format!(
                "{}{}",
                self.db_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                suffix
            ));
            if let Ok(meta) = std::fs::metadata(&p) {
                if let Ok(mtime) = meta.modified() {
                    stamp = stamp.max(
                        mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0.0, |d| d.as_secs_f64()),
                    );
                }
            }
        }
        stamp
    }

    fn invalidate(&self) {
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.session_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.calls_index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// One indexed probe serving both caches — `None` = unknown id.
    fn session_stamp(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<SessionStamp>, rusqlite::Error> {
        let tool_state = if self.has_tool_call_state {
            "(select count(*) from tool_call_state where session_id = s.id)"
        } else {
            "0"
        };
        let sql = format!(
            "select s.last_activity_at, s.backend_type,
                    (select count(*) from message_nodes where session_id = s.id),
                    (select count(*) from prompt_history where session_id = s.id),
                    {tool_state},
                    (select coalesce(max(node_id), 0) from message_nodes where session_id = s.id)
             from sessions s where s.id = ?"
        );
        conn.query_row(&sql, params![id], |r| {
            Ok(SessionStamp {
                last_activity_at: r.get(0)?,
                backend_type: r.get(1)?,
                node_count: r.get(2)?,
                prompt_count: r.get(3)?,
                tool_state_count: r.get(4)?,
                max_node_id: r.get(5)?,
            })
        })
        .optional()
    }

    /// `subagent_heads`: `agent_id` = spawned session's id; `session_id` =
    /// the session that spawned it.
    fn parent_session_id(&self, conn: &Connection, id: &str) -> Option<String> {
        if !self.has_subagent_heads {
            return None;
        }
        conn.query_row(
            "select session_id from subagent_heads where agent_id = ? limit 1",
            params![id],
            |r| r.get(0),
        )
        .ok()
    }

    fn subagent_parents(&self, conn: &Connection) -> HashMap<String, String> {
        let mut parents = HashMap::new();
        if !self.has_subagent_heads {
            return parents;
        }
        if let Ok(mut stmt) = conn.prepare("select session_id, agent_id from subagent_heads") {
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    parents.insert(row.1, row.0);
                }
            }
        }
        parents
    }

    /// `tool_call_state` rows: serialised ACP `ToolCall`/`ToolCallUpdate`;
    /// the update carries the authoritative `status` plus
    /// `_meta["cognition.ai/terminal_exit"]`. Malformed rows are skipped.
    fn tool_call_state_outcomes(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> HashMap<String, ToolCallOutcome> {
        let mut outcomes = HashMap::new();
        if !self.has_tool_call_state {
            return outcomes;
        }
        let Ok(mut stmt) =
            conn.prepare("select tool_call_update_json from tool_call_state where session_id = ?")
        else {
            return outcomes;
        };
        let rows = stmt.query_map(params![session_id], |r| r.get::<_, Option<String>>(0));
        let Ok(rows) = rows else { return outcomes };
        for row in rows.flatten().flatten() {
            let Ok(update) = serde_json::from_str::<Value>(&row) else {
                continue;
            };
            let Some(id) = update["toolCallId"].as_str() else {
                continue;
            };
            let Some(status) = mapping::from_acp_tool_call_status(update["status"].as_str()) else {
                continue;
            };
            let exit_code = update["_meta"]["cognition.ai/terminal_exit"]["exit_code"].as_i64();
            outcomes.insert(
                id.to_string(),
                ToolCallOutcome {
                    status,
                    exit_code,
                    duration_ms: None,
                },
            );
        }
        outcomes
    }

    fn parse_node_row(
        node_id: i64,
        parent_node_id: Option<i64>,
        chat_message: &str,
        metadata: Option<&str>,
        created_at: f64,
    ) -> MessageNode {
        let msg = mapping::parse_json_or(Some(chat_message), Value::Null);
        let meta = mapping::parse_json_or(metadata, Value::Null);
        mapping::parse_chat_message(&msg, &meta, node_id, parent_node_id, created_at)
    }

    fn build_session(
        &self,
        conn: &Connection,
        row: &mapping::SessionRow,
        id: &str,
    ) -> Result<Session, StorageError> {
        let mut stmt = conn
            .prepare(
                "select node_id, parent_node_id, chat_message, created_at, metadata
                 from message_nodes where session_id = ? order by node_id",
            )
            .map_err(err("Failed to parse session"))?;
        let node_rows: Vec<NodeRow> = stmt
            .query_map(params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .map_err(err("Failed to parse session"))?
            .collect::<Result<_, _>>()
            .map_err(err("Failed to parse session"))?;
        let nodes: Vec<MessageNode> = node_rows
            .iter()
            .map(|(nid, pid, msg, created, meta)| {
                Self::parse_node_row(*nid, *pid, msg, meta.as_deref(), *created)
            })
            .collect();

        let mut stmt = conn
            .prepare(
                "select content, timestamp, is_shell from prompt_history
                 where session_id = ? order by id",
            )
            .map_err(err("Failed to parse session"))?;
        let prompts: Vec<PromptHistoryEntry> = stmt
            .query_map(params![id], |r| {
                Ok(PromptHistoryEntry {
                    content: r.get(0)?,
                    timestamp: r.get(1)?,
                    is_shell: r.get::<_, i64>(2)? == 1,
                })
            })
            .map_err(err("Failed to parse session"))?
            .collect::<Result<_, _>>()
            .map_err(err("Failed to parse session"))?;

        // Tool nodes carry each call's recorded outcome; `tool_call_state`
        // (when present) is the authoritative lifecycle and overrides them.
        let mut outcomes = shared::tool_node_outcomes(&nodes);
        for (id, outcome) in self.tool_call_state_outcomes(conn, id) {
            let merged = outcomes.get(&id).cloned().unwrap_or(ToolCallOutcome {
                status: ToolCallStatus::Pending,
                exit_code: None,
                duration_ms: None,
            });
            outcomes.insert(
                id,
                ToolCallOutcome {
                    status: outcome.status,
                    exit_code: outcome.exit_code.or(merged.exit_code),
                    duration_ms: outcome.duration_ms.or(merged.duration_ms),
                },
            );
        }
        let nodes = shared::apply_tool_call_outcomes(&nodes, &outcomes);

        Ok(mapping::session_from_devin_row(
            row,
            nodes,
            prompts,
            self.parent_session_id(conn, id),
            None,
        ))
    }

    fn session_row(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<mapping::SessionRow>, rusqlite::Error> {
        conn.query_row(
            "select id, working_directory, backend_type, model, agent_mode, created_at,
                    last_activity_at, title, main_chain_id, shell_last_seen_index,
                    cogs_json, workspace_dirs, hidden, metadata
             from sessions where id = ?",
            params![id],
            |r| {
                Ok(mapping::SessionRow {
                    id: r.get(0)?,
                    working_directory: r.get(1)?,
                    backend_type: r.get(2)?,
                    model: r.get(3)?,
                    agent_mode: r.get(4)?,
                    created_at: r.get(5)?,
                    last_activity_at: r.get(6)?,
                    title: r.get(7)?,
                    main_chain_id: r.get(8)?,
                    shell_last_seen_index: r.get(9)?,
                    cogs_json: r.get(10)?,
                    workspace_dirs: r.get(11)?,
                    hidden: r.get(12)?,
                    metadata: r.get(13)?,
                })
            },
        )
        .optional()
    }

    /// Summary rows projected without `cogs_json` (megabytes per row) and
    /// without `message_nodes` entirely.
    fn summary_rows(conn: &Connection) -> Result<Vec<mapping::SessionRow>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "select id, working_directory, backend_type, model, agent_mode, created_at,
                    last_activity_at, title, main_chain_id, shell_last_seen_index,
                    workspace_dirs, hidden, metadata
             from sessions order by last_activity_at desc",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(mapping::SessionRow {
                id: r.get(0)?,
                working_directory: r.get(1)?,
                backend_type: r.get(2)?,
                model: r.get(3)?,
                agent_mode: r.get(4)?,
                created_at: r.get(5)?,
                last_activity_at: r.get(6)?,
                title: r.get(7)?,
                main_chain_id: r.get(8)?,
                shell_last_seen_index: r.get(9)?,
                cogs_json: None,
                workspace_dirs: r.get(10)?,
                hidden: r.get(11)?,
                metadata: r.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Rewind write — `truncateSessionNodes` port: delete the removed
    /// `message_nodes` plus their `tool_call_state`/`subagent_heads`
    /// hangers-on; move `last_activity_at`/`main_chain_id` back.
    /// `prompt_history` is deliberately left alone (no join key exists).
    ///
    /// # Errors
    /// `StorageError` on sqlite failure.
    pub fn truncate_session_nodes(
        &self,
        session_id: &str,
        removed_node_ids: &[i64],
        removed_tool_call_ids: &[String],
        last_activity_at: f64,
        main_chain_id: i64,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = conn
            .unchecked_transaction()
            .map_err(err("Failed to truncate session"))?;
        if !removed_node_ids.is_empty() {
            let marks = removed_node_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = tx
                .prepare(&format!(
                    "DELETE FROM message_nodes WHERE session_id = ?1 AND node_id IN ({marks})"
                ))
                .map_err(err("Failed to truncate session"))?;
            let mut sql_params: Vec<rusqlite::types::Value> = vec![session_id.to_string().into()];
            sql_params.extend(removed_node_ids.iter().map(|id| (*id).into()));
            stmt.execute(rusqlite::params_from_iter(sql_params))
                .map_err(err("Failed to truncate session"))?;
            if self.has_subagent_heads {
                let mut stmt = tx
                    .prepare(&format!(
                        "DELETE FROM subagent_heads WHERE session_id = ?1 AND chain_node_id IN ({marks})"
                    ))
                    .map_err(err("Failed to truncate session"))?;
                let mut sql_params: Vec<rusqlite::types::Value> =
                    vec![session_id.to_string().into()];
                sql_params.extend(removed_node_ids.iter().map(|id| (*id).into()));
                stmt.execute(rusqlite::params_from_iter(sql_params))
                    .map_err(err("Failed to truncate session"))?;
            }
        }
        if self.has_tool_call_state && !removed_tool_call_ids.is_empty() {
            let marks = removed_tool_call_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = tx
                .prepare(&format!(
                    "DELETE FROM tool_call_state WHERE session_id = ?1 AND tool_call_id IN ({marks})"
                ))
                .map_err(err("Failed to truncate session"))?;
            let mut sql_params: Vec<rusqlite::types::Value> = vec![session_id.to_string().into()];
            sql_params.extend(removed_tool_call_ids.iter().map(|id| id.clone().into()));
            stmt.execute(rusqlite::params_from_iter(sql_params))
                .map_err(err("Failed to truncate session"))?;
        }
        tx.execute(
            "UPDATE sessions SET last_activity_at = ?1, main_chain_id = ?2 WHERE id = ?3",
            params![last_activity_at, main_chain_id, session_id],
        )
        .map_err(err("Failed to truncate session"))?;
        tx.commit().map_err(err("Failed to truncate session"))?;
        drop(conn);
        self.invalidate();
        Ok(())
    }
}

trait Optional<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> Optional<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn table_names(conn: &Connection) -> Result<BTreeSet<String>, StorageError> {
    let mut stmt = conn
        .prepare("select name from sqlite_master where type = 'table'")
        .map_err(err("Failed to open database"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err("Failed to open database"))?;
    rows.collect::<Result<_, _>>()
        .map_err(err("Failed to open database"))
}

#[async_trait]
impl SessionRepository for DevinStore {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        if self.readonly {
            return Err(StorageError::new(format!(
                "Store is open read-only, cannot save session {}",
                session.id
            )));
        }
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = conn
            .unchecked_transaction()
            .map_err(err("Failed to save session"))?;
        tx.execute(
            "DELETE FROM message_nodes WHERE session_id = ?",
            params![session.id],
        )
        .map_err(err("Failed to save session"))?;
        tx.execute(
            "DELETE FROM prompt_history WHERE session_id = ?",
            params![session.id],
        )
        .map_err(err("Failed to save session"))?;
        tx.execute("DELETE FROM sessions WHERE id = ?", params![session.id])
            .map_err(err("Failed to save session"))?;

        // Checkpoint refs ride the session metadata under a sepia-namespaced
        // key so an imported session keeps its snapshot pointers.
        let mut session_meta = match &session.metadata {
            Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        if !session.checkpoints.is_empty() {
            session_meta.insert(
                shared::SESSION_CHECKPOINTS_KEY.into(),
                serde_json::to_value(&session.checkpoints).unwrap_or_default(),
            );
        }

        tx.execute(
            "INSERT INTO sessions (id, working_directory, backend_type, model, agent_mode,
             created_at, last_activity_at, title, main_chain_id, shell_last_seen_index,
             cogs_json, workspace_dirs, hidden, metadata)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                session.id,
                session.working_directory,
                session.backend_type,
                session.model,
                session.agent_mode,
                session.created_at,
                session.last_activity_at,
                session.title,
                session.main_chain_id,
                session.shell_last_seen_index,
                session.cogs_json,
                session.workspace_dirs,
                session.hidden,
                serde_json::to_string(&session_meta).unwrap_or_default(),
            ],
        )
        .map_err(err("Failed to save session"))?;

        for node in &session.nodes {
            tx.execute(
                "INSERT INTO message_nodes (session_id, node_id, parent_node_id, chat_message, created_at, metadata)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    session.id,
                    node.node_id,
                    node.parent_node_id,
                    serde_json::to_string(&mapping::build_chat_message(node, &session.model))
                        .unwrap_or_default(),
                    node.created_at,
                    if node.metadata.is_null() {
                        None
                    } else {
                        Some(serde_json::to_string(&node.metadata).unwrap_or_default())
                    },
                ],
            )
            .map_err(err("Failed to save session"))?;
        }
        for ph in &session.prompt_history {
            tx.execute(
                "INSERT INTO prompt_history (session_id, content, timestamp, is_shell)
                 VALUES (?, ?, ?, ?)",
                params![session.id, ph.content, ph.timestamp, i64::from(ph.is_shell)],
            )
            .map_err(err("Failed to save session"))?;
        }
        tx.commit().map_err(err("Failed to save session"))?;
        drop(conn);
        self.invalidate();
        Ok(())
    }

    async fn get_by_id(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stamp) = self
            .session_stamp(&conn, id)
            .map_err(err("Failed to read session"))?
        else {
            return Ok(None);
        };
        let key = stamp.cache_key();
        if let Some((k, session)) = self
            .session_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
        {
            if k == &key {
                return Ok(Some(session.clone()));
            }
        }
        let Some(row) = Self::session_row(&conn, id).map_err(err("Failed to read session"))? else {
            return Ok(None);
        };
        let session = self.build_session(&conn, &row, id)?;

        // Giant sessions (>32 MB of blobs) still serve but skip retention —
        // pinning one is an OOM, not a cache.
        let mut stmt = conn
            .prepare(
                "select coalesce(sum(length(chat_message) + coalesce(length(metadata),0)),0)
                      from message_nodes where session_id = ?",
            )
            .map_err(err("Failed to read session"))?;
        let blob_bytes: i64 = stmt
            .query_row(params![id], |r| r.get(0))
            .map_err(err("Failed to read session"))?;
        if usize::try_from(blob_bytes.max(0)).unwrap_or(usize::MAX) <= SESSION_CACHE_MAX_BYTES {
            let mut cache = self
                .session_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.len() >= SESSION_CACHE_MAX {
                cache.clear();
            }
            cache.insert(id.to_string(), (key, session.clone()));
        }
        Ok(Some(session))
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let stamp = self.db_stamp();
        if let Some((s, sessions)) = &*self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            if s.to_bits() == stamp.to_bits() {
                return Ok(sessions.clone());
            }
        }
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rows = Self::summary_rows(&conn).map_err(err("Failed to list sessions"))?;
        let parents = self.subagent_parents(&conn);
        let mut sessions = Vec::with_capacity(rows.len());
        for row in &rows {
            // Unreadable rows are skipped, not fatal.
            sessions.push(mapping::session_from_devin_row(
                &mapping::SessionRow {
                    cogs_json: Some("[]".into()),
                    ..row.clone()
                },
                Vec::new(),
                Vec::new(),
                parents.get(&row.id).cloned(),
                None,
            ));
        }
        drop(conn);
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((stamp, sessions.clone()));
        Ok(sessions)
    }

    async fn summary(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(row) = Self::session_row(&conn, id).map_err(err("Failed to read session"))? else {
            return Ok(None);
        };
        Ok(Some(mapping::session_from_devin_row(
            &row,
            Vec::new(),
            Vec::new(),
            self.parent_session_id(&conn, id),
            None,
        )))
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stamp) = self
            .session_stamp(&conn, id)
            .map_err(err("Failed to page session nodes"))?
        else {
            return Ok(None);
        };
        let total = usize::try_from(stamp.node_count.max(0)).unwrap_or(usize::MAX);
        let before = options
            .before
            .map_or(total, |b| (b.max(0) as usize).min(total));
        let limit = options.limit.unwrap_or(total).max(1);
        let start = before.saturating_sub(limit);

        // Warm whole-session cache → slice in memory.
        if let Some((k, session)) = self
            .session_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
        {
            if *k == stamp.cache_key() {
                let nodes: Vec<MessageNode> = session
                    .nodes
                    .iter()
                    .skip(start)
                    .take(before - start)
                    .cloned()
                    .collect();
                let tool_call_nodes: Vec<MessageNode> = session
                    .nodes
                    .iter()
                    .filter(|n| !n.tool_calls.is_empty())
                    .cloned()
                    .collect();
                return Ok(Some(SessionNodeWindow {
                    nodes,
                    tool_call_nodes,
                    total,
                    start,
                    backend_type: stamp.backend_type.clone(),
                }));
            }
        }

        let mut stmt = conn
            .prepare(
                "select node_id, parent_node_id, chat_message, created_at, metadata
                 from message_nodes where session_id = ? order by node_id limit ? offset ?",
            )
            .map_err(err("Failed to page session nodes"))?;
        let window_rows: Vec<NodeRow> = stmt
            .query_map(
                params![
                    id,
                    i64::try_from(before - start).unwrap_or(i64::MAX),
                    i64::try_from(start).unwrap_or(i64::MAX)
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(err("Failed to page session nodes"))?
            .collect::<Result<_, _>>()
            .map_err(err("Failed to page session nodes"))?;
        let nodes: Vec<MessageNode> = window_rows
            .iter()
            .map(|(nid, pid, msg, created, meta)| {
                Self::parse_node_row(*nid, *pid, msg, meta.as_deref(), *created)
            })
            .collect();
        let mut tool_call_nodes: Vec<MessageNode> = nodes
            .iter()
            .filter(|n| !n.tool_calls.is_empty())
            .cloned()
            .collect();

        // Resolve calls only when the window actually has tool rows.
        if nodes.iter().any(|n| n.role == sepia_core::Role::Tool) {
            let in_window: HashSet<i64> = nodes.iter().map(|n| n.node_id).collect();
            let fresh = self
                .calls_index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id)
                .is_some_and(|c| {
                    c.max_node_id == stamp.max_node_id || c.built_at.elapsed() < CALLS_INDEX_TTL
                });
            if fresh {
                let index = self
                    .calls_index
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(entry) = index.get(id) {
                    tool_call_nodes.extend(
                        entry
                            .nodes
                            .iter()
                            .filter(|n| !in_window.contains(&n.node_id))
                            .cloned(),
                    );
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        "select node_id, parent_node_id, chat_message, created_at, metadata,
                                length(chat_message) + coalesce(length(metadata), 0) as bytes
                         from message_nodes where session_id = ?
                           and json_array_length(chat_message, '$.tool_calls') > 0",
                    )
                    .map_err(err("Failed to page session nodes"))?;
                let call_rows: Vec<(i64, Option<i64>, String, f64, Option<String>, i64)> = stmt
                    .query_map(params![id], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    })
                    .map_err(err("Failed to page session nodes"))?
                    .collect::<Result<_, _>>()
                    .map_err(err("Failed to page session nodes"))?;
                let call_bytes: i64 = call_rows.iter().map(|r| r.5).sum();
                if usize::try_from(call_bytes.max(0)).unwrap_or(usize::MAX)
                    <= SESSION_CACHE_MAX_BYTES
                {
                    let parsed: Vec<MessageNode> = call_rows
                        .iter()
                        .filter(|(nid, ..)| !in_window.contains(nid))
                        .map(|(nid, pid, msg, created, meta, _)| {
                            Self::parse_node_row(*nid, *pid, msg, meta.as_deref(), *created)
                        })
                        .filter(|n| !n.tool_calls.is_empty())
                        .collect();
                    let mut index = self
                        .calls_index
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if index.len() >= SESSION_CACHE_MAX {
                        index.clear();
                    }
                    index.insert(
                        id.to_string(),
                        CallsEntry {
                            max_node_id: stamp.max_node_id,
                            built_at: Instant::now(),
                            nodes: parsed.clone(),
                        },
                    );
                    tool_call_nodes.extend(parsed);
                }
            }
        }

        Ok(Some(SessionNodeWindow {
            nodes,
            tool_call_nodes,
            total,
            start,
            backend_type: stamp.backend_type.clone(),
        }))
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        if self.readonly {
            return Err(StorageError::new(format!(
                "Store is open read-only, cannot delete session {id}"
            )));
        }
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = conn
            .unchecked_transaction()
            .map_err(err("Failed to delete session"))?;
        tx.execute(
            "DELETE FROM message_nodes WHERE session_id = ?",
            params![id],
        )
        .map_err(err("Failed to delete session"))?;
        tx.execute(
            "DELETE FROM prompt_history WHERE session_id = ?",
            params![id],
        )
        .map_err(err("Failed to delete session"))?;
        tx.execute("DELETE FROM sessions WHERE id = ?", params![id])
            .map_err(err("Failed to delete session"))?;
        tx.commit().map_err(err("Failed to delete session"))?;
        drop(conn);
        self.invalidate();
        Ok(())
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(self
            .session_stamp(&conn, id)
            .map_err(err("Failed to check session"))?
            .is_some())
    }
}
