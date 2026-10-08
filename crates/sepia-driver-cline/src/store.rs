//! `ClineRepository.ts` port — the `SessionRepository` over Cline's
//! on-disk session dirs under `<dataDir>/sessions/<id>/`.
//!
//! Reads mirror the TS exactly: `list` summarizes manifests behind a
//! dir-stamp cache, `get_by_id` parses the full transcript. The write side
//! is where the driver diverges from the TS repository (which is
//! read-only): `save` runs the `ClineStore.install` pipeline — manifest +
//! transcript pair plus the `db/sessions.db` index row — and embeds the
//! full IR under a `sepia` key in the transcript so a `save` round-trips
//! faithfully. `delete` removes the session dir and its index row.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rusqlite::params;
use sepia_core::domain::{MessageNode, Session, StorageError};
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use serde_json::{Map, Value, json};

use crate::cline;
use crate::cline_index;

/// Embedded-IR key inside the messages file — `data["sepia"]["session"]`
/// carries the wire payload `save` wrote, so `get_by_id` can return the
/// session exactly (the native pair is lossy by design: it synthesizes
/// system/user seed nodes on every read).
const SIDECAR_KEY: &str = "sepia";

/// A `SessionRepository` over `<dataDir>/sessions/` — `data_dir` is
/// usually `~/.cline/data`.
pub struct ClineStore {
    data_dir: PathBuf,
    /// Read cache — `list` reads and parses every session's manifest on
    /// each call. The directory walk is cheap, so the parsed array is
    /// keyed on a stamp of it (mirrors the TS mtime/file-listing stamp).
    list_cache: Mutex<Option<(String, Vec<Session>)>>,
}

fn storage_err(prefix: &str) -> impl Fn(std::io::Error) -> StorageError + '_ {
    move |cause| StorageError::new(format!("{prefix}: {cause}"))
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_manifest(name: &str) -> bool {
    name.ends_with(".json") && !name.ends_with(".messages.json") && !name.contains(".compaction.")
}

fn file_mtime_ms(path: &Path) -> Option<f64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64() * 1000.0)
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
        .floor()
}

/// `new Date(value).getTime()/1000` for the manifest's ISO strings —
/// anything unparseable is `None`, exactly like `toSeconds` in the TS.
fn to_seconds(value: &Value) -> Option<f64> {
    value
        .as_str()
        .and_then(cline::iso_to_seconds)
        .map(f64::floor)
}

/// `prompt.replace(/<[^>]+>/g, " ")` — `<`…`>` spans become a space;
/// unmatched `<`/`>` survive verbatim.
fn strip_angle_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        match after.find('>') {
            // `<[^>]+>` needs at least one char between the angles.
            Some(gt) if gt >= 1 => {
                out.push(' ');
                rest = &after[gt + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn title_of(meta: &Value, fallback: &str) -> String {
    if let Some(metadata) = meta.get("metadata").and_then(Value::as_object) {
        if let Some(title) = metadata.get("title").and_then(Value::as_str) {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if let Some(prompt) = meta.get("prompt").and_then(Value::as_str) {
        let text = strip_angle_tags(prompt);
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return collapsed.chars().take(80).collect();
        }
    }
    fallback.to_string()
}

/// Manifest-only Session for `list`; `get_by_id` loads the real transcript.
fn manifest_to_session(raw: &str, fallback_id: &str) -> Option<Session> {
    let meta: Value = serde_json::from_str(raw).ok()?;
    let get = |key: &str| meta.get(key).unwrap_or(&Value::Null);
    let id = get("session_id")
        .as_str()
        .map_or_else(|| fallback_id.to_string(), str::to_string);
    let created_at = to_seconds(get("started_at")).unwrap_or_else(now_seconds);
    let subagent = cline::cline_subagent_info(&id);
    Some(Session {
        id: id.clone(),
        title: title_of(&meta, &id),
        working_directory: get("cwd").as_str().unwrap_or("/").to_string(),
        backend_type: "cline".into(),
        agent_mode: "accept-edits".into(),
        model: get("model").as_str().map_or_else(
            || "unknown".to_string(),
            |m| m.strip_prefix("cline-pass/").unwrap_or(m).to_string(),
        ),
        created_at,
        last_activity_at: to_seconds(get("ended_at")).unwrap_or(created_at),
        main_chain_id: 0,
        shell_last_seen_index: 0,
        cogs_json: "[]".into(),
        workspace_dirs: "[]".into(),
        hidden: 0,
        parent_session_id: subagent.as_ref().map(|s| s.parent_session_id.clone()),
        agent_id: subagent.as_ref().map(|s| s.agent_id.clone()),
        checkpoints: cline::checkpoints_from_manifest(&meta),
        metadata: json!({}),
        nodes: Vec::new(),
        prompt_history: Vec::new(),
    })
}

/// The `clineMessageIndex` a reader-tagged node carries; anything else is unmappable.
fn source_index_of(node: &MessageNode) -> Option<i64> {
    let index = node.metadata.as_object()?.get("clineMessageIndex")?;
    let n = index.as_f64().filter(|n| n.is_finite())?;
    if n.fract() == 0.0 && n >= 0.0 {
        Some(n as i64)
    } else {
        None
    }
}

/// `path.resolve` — absolutize then fold `.`/`..` lexically (no symlink walk).
fn resolve_path(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

impl ClineStore {
    /// A repository over `data_dir` (`~/.cline/data` in production).
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            list_cache: Mutex::new(None),
        }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.data_dir.join("sessions")
    }

    fn db_path(&self) -> PathBuf {
        self.data_dir.join("db").join("sessions.db")
    }

    fn invalidate(&self) {
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// The `{status, pid}` index row for `id` — `None` when the index is
    /// absent or has no row.
    fn index_row(&self, id: &str) -> Result<Option<(String, i64)>, StorageError> {
        let db_path = self.db_path();
        if !db_path.exists() {
            return Ok(None);
        }
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| StorageError::new(format!("Failed to read the Cline session index: {e}")))?;
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("select name from sqlite_master where type = 'table'")
                .map_err(|e| {
                    StorageError::new(format!("Failed to read the Cline session index: {e}"))
                })?;
            stmt.query_map([], |r| r.get(0))
                .map_err(|e| {
                    StorageError::new(format!("Failed to read the Cline session index: {e}"))
                })?
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    StorageError::new(format!("Failed to read the Cline session index: {e}"))
                })?
        };
        if !tables.iter().any(|t| t == "sessions") {
            return Ok(None);
        }
        let row = conn
            .query_row(
                "select status, pid from sessions where session_id = ?",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .ok();
        Ok(row)
    }

    /// Insert or replace the session's index row, creating the index the
    /// way the CLI's own DDL does.
    fn register(
        &self,
        session: &Session,
        id: &str,
        messages_path: &Path,
    ) -> Result<(), StorageError> {
        let db_dir = self.data_dir.join("db");
        std::fs::create_dir_all(&db_dir)
            .map_err(storage_err("Failed to register the cline session"))?;
        let db_path = db_dir.join("sessions.db");
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| StorageError::new(format!("Failed to register the cline session: {e}")))?;
        conn.execute_batch(cline_index::SESSIONS_DDL)
            .map_err(|e| StorageError::new(format!("Failed to register the cline session: {e}")))?;
        let now = cline::to_iso_ms(
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64()),
        );
        let row = cline_index::session_row(session, id, &messages_path.to_string_lossy(), &now);
        let values: Vec<rusqlite::types::Value> = cline_index::SESSION_COLUMNS
            .iter()
            .map(|col| match row.get(*col).unwrap_or(&Value::Null) {
                Value::Null => rusqlite::types::Value::Null,
                Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
                Value::Number(n) => n.as_i64().map_or_else(
                    || rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
                    rusqlite::types::Value::Integer,
                ),
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                other => rusqlite::types::Value::Text(other.to_string()),
            })
            .collect();
        conn.execute(
            &cline_index::insert_session_sql(),
            rusqlite::params_from_iter(values),
        )
        .map_err(|e| StorageError::new(format!("Failed to register the cline session: {e}")))?;
        Ok(())
    }

    /// The embedded IR `save` stored in `<id>.messages.json`, or `None`
    /// for a native Cline session (whose transcript parses through
    /// `from_directory` instead).
    fn read_sidecar(&self, id: &str) -> Option<Session> {
        let path = self
            .sessions_dir()
            .join(id)
            .join(format!("{id}.messages.json"));
        let raw = std::fs::read_to_string(path).ok()?;
        let data: Value = serde_json::from_str(&raw).ok()?;
        let embedded = data.get(SIDECAR_KEY)?.get("session")?;
        let mut session = sepia_core::wire::session_from_json(embedded).ok()?;
        // The repository stamps its own backend on every read, sidecar or not.
        session.backend_type = "cline".into();
        Some(session)
    }

    /// In-place conversation rewind: slice the transcript's `messages`
    /// array just past the last kept node's source entry.
    ///
    /// Each node emitted by `cline::from_directory` records its source
    /// position in `metadata.clineMessageIndex`, so the cut lands on whole
    /// messages — nodes sharing one entry (assistant twins, a multi-result
    /// turn) survive or drop together even when the requested boundary
    /// lands between them. Every other top-level field of the messages
    /// file (`version`, `agent`, `origin`, fields the reader never
    /// modeled) is preserved verbatim and the manifest is never touched —
    /// strictly less invasive than a `ClineStore.install` rebuild, which
    /// regenerates the whole pair from IR.
    ///
    /// Refuses when the resolved `messages_path` escapes the data dir or
    /// the file provably belongs to another session (a subagent manifest
    /// pointing at the parent's transcript would truncate the parent's
    /// history).
    ///
    /// # Errors
    /// `StorageError` on fs failures, malformed JSON, or the refusal paths
    /// documented above.
    pub fn truncate_session(
        &self,
        session_id: &str,
        kept: &[MessageNode],
        removed: &[MessageNode],
    ) -> Result<i64, StorageError> {
        let fail = |message: String| StorageError::new(message);
        let dir = self.sessions_dir().join(session_id);

        // Resolve the transcript the way `cline::from_directory` does:
        // manifest's `messages_path`, else the `<id>.messages.json` sibling.
        let entries =
            std::fs::read_dir(&dir).map_err(storage_err("Failed to truncate cline session"))?;
        let mut meta_name: Option<String> = None;
        for entry in entries {
            let entry = entry.map_err(storage_err("Failed to truncate cline session"))?;
            let name = entry.file_name().to_string_lossy().to_string();
            if is_manifest(&name) {
                meta_name = Some(name);
                break;
            }
        }
        let Some(meta_name) = meta_name else {
            return Err(fail(format!(
                "No session metadata json found in {}",
                dir.display()
            )));
        };
        let base = meta_name.strip_suffix(".json").unwrap_or(&meta_name);
        let meta_raw = std::fs::read_to_string(dir.join(&meta_name))
            .map_err(storage_err("Failed to truncate cline session"))?;
        let meta: Value = serde_json::from_str(&meta_raw)
            .map_err(|_| fail(format!("Cline manifest is not valid JSON: {meta_name}")))?;
        let declared_path = meta
            .get("messages_path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty());
        let resolved = resolve_path(
            &declared_path.map_or_else(|| dir.join(format!("{base}.messages.json")), PathBuf::from),
        );
        let root = resolve_path(&self.data_dir);
        if resolved != root && !resolved.starts_with(&root) {
            return Err(fail(format!(
                "Cline messages_path escapes the data dir: {}",
                declared_path.unwrap_or("")
            )));
        }

        let raw = std::fs::read_to_string(&resolved)
            .map_err(storage_err("Failed to truncate cline session"))?;
        let data: Value = serde_json::from_str(&raw).map_err(|_| {
            fail(format!(
                "Cline transcript is not valid JSON: {}",
                resolved.display()
            ))
        })?;
        let messages_len = {
            let Some(messages) = data.get("messages").and_then(Value::as_array) else {
                return Err(fail(format!(
                    "Cline transcript carries no message array: {}",
                    resolved.display()
                )));
            };
            messages.len()
        };
        // The file must belong to this session — a manifest pointing at a
        // transcript that names another id would cut that session's history.
        let file_session_id = data.get("sessionId").and_then(Value::as_str).or_else(|| {
            data.get("origin")
                .and_then(|o| o.get("sessionId"))
                .and_then(Value::as_str)
        });
        if let Some(file_id) = file_session_id {
            if file_id != session_id {
                return Err(fail(format!(
                    "Cline transcript belongs to {file_id}, not {session_id}"
                )));
            }
        } else {
            let basename = resolved
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().to_string());
            if basename != format!("{session_id}.messages.json") {
                return Err(fail(format!(
                    "Cannot prove {} is {session_id}'s transcript — refusing to truncate",
                    resolved.display()
                )));
            }
        }

        let kept_indices: Vec<i64> = kept.iter().filter_map(source_index_of).collect();
        for node in removed {
            if source_index_of(node).is_none() {
                return Err(fail(format!(
                    "Node {} has no recorded transcript entry — cannot truncate",
                    node.node_id
                )));
            }
        }
        let cut_index = kept_indices.iter().max().map_or(0, |m| m + 1);
        let cut_index = usize::try_from(cut_index.max(0)).unwrap_or(usize::MAX);
        if cut_index >= messages_len {
            return Ok(0);
        }

        let now = cline::to_iso_ms(
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64()),
        );
        let mut out = data.as_object().cloned().unwrap_or_default();
        out.insert("updated_at".into(), json!(now));
        let messages = data
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        out.insert(
            "messages".into(),
            Value::Array(messages[..cut_index].to_vec()),
        );
        // The embedded `sepia` IR would go stale past the cut — drop it so
        // reads fall back to the (now truncated) transcript itself.
        out.remove(SIDECAR_KEY);
        let removed_messages = messages_len - cut_index;

        // tmp + rename: a crash mid-write must not leave a torn transcript.
        let tmp = PathBuf::from(format!("{}.tmp-{}", resolved.display(), std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default(),
        )
        .map_err(storage_err("Failed to truncate cline session"))?;
        std::fs::rename(&tmp, &resolved)
            .map_err(storage_err("Failed to truncate cline session"))?;
        self.invalidate();
        Ok(i64::try_from(removed_messages).unwrap_or(i64::MAX))
    }
}

impl ClineStore {
    /// `save` under a caller-chosen id; `force` overwrites a live-owned
    /// index row (the `--force` install path).
    ///
    /// # Errors
    /// `StorageError` on live-owner refusal (without `force`) or I/O.
    pub async fn install(
        &self,
        session: &Session,
        id: &str,
        force: bool,
    ) -> Result<(), StorageError> {
        let dir = self.sessions_dir().join(id);
        let messages_path = dir.join(format!("{id}.messages.json"));

        if !force {
            if let Some((status, pid)) = self.index_row(id)? {
                if cline_index::is_active_row(&status, cline_index::is_pid_alive(pid)) {
                    return Err(StorageError::new(format!(
                        "Session {id} still belongs to a live owner (status {status}, pid {pid}); resume it or retry with --force"
                    )));
                }
            }
        }
        self.install_inner(session, id, &dir, &messages_path).await
    }

    async fn install_inner(
        &self,
        session: &Session,
        id: &str,
        dir: &std::path::Path,
        messages_path: &std::path::Path,
    ) -> Result<(), StorageError> {
        std::fs::create_dir_all(dir).map_err(storage_err("Cline install failed"))?;
        let manifest = cline::session_manifest(session, id, &messages_path.to_string_lossy());
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .map_err(storage_err("Cline install failed"))?;

        // The transcript embeds the full IR — `get_by_id` prefers it so a
        // saved session round-trips without the lossy re-parse.
        let mut messages = cline::session_messages(session, id);
        if let Value::Object(map) = &mut messages {
            let sidecar = sepia_core::wire::session_to_json(session).unwrap_or_default();
            let mut sepia = Map::new();
            sepia.insert("session".into(), sidecar);
            map.insert(SIDECAR_KEY.into(), Value::Object(sepia));
        }
        std::fs::write(
            messages_path,
            serde_json::to_string_pretty(&messages).unwrap_or_default(),
        )
        .map_err(storage_err("Cline install failed"))?;

        self.register(session, id, messages_path)?;
        self.invalidate();
        Ok(())
    }
}

#[async_trait]
impl SessionRepository for ClineStore {
    /// `ClineStore.install` — write the manifest/transcript pair and the
    /// `db/sessions.db` index row. Refuses to overwrite a session that
    /// still belongs to a live owner.
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        self.install(session, &session.id, false).await
    }

    async fn get_by_id(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        if let Some(session) = self.read_sidecar(id) {
            return Ok(Some(session));
        }
        match cline::from_directory(&self.sessions_dir().join(id), None) {
            Ok(mut session) => {
                session.backend_type = "cline".into();
                Ok(Some(session))
            }
            Err(e) if e.message.to_lowercase().contains("not found") => Ok(None),
            Err(e) => Err(StorageError::new(e.message)),
        }
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let root = self.sessions_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let entries =
            std::fs::read_dir(&root).map_err(storage_err("Failed to list cline sessions"))?;
        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(storage_err("Failed to list cline sessions"))?;
            names.push(entry.file_name().to_string_lossy().to_string());
        }

        let mut manifests: Vec<(PathBuf, String)> = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        for entry in &names {
            let dir = root.join(entry);
            let dir_mtime = file_mtime_ms(&dir).map_or_else(|| "?".to_string(), |m| m.to_string());
            let info = std::fs::metadata(&dir).ok();
            if info.is_none_or(|i| !i.is_dir()) {
                parts.push(format!("{entry}:{dir_mtime}"));
                continue;
            }
            let files: Vec<String> = std::fs::read_dir(&dir).map_or_else(
                |_| Vec::new(),
                |rd| {
                    rd.filter_map(std::result::Result::ok)
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                },
            );
            let meta_name = files.iter().find(|f| is_manifest(f));
            let meta_mtime = meta_name.map_or_else(String::new, |m| {
                file_mtime_ms(&dir.join(m)).map_or_else(|| "?".to_string(), |t| t.to_string())
            });
            parts.push(format!(
                "{entry}:{dir_mtime}:{}:{}:{meta_mtime}",
                files.join(","),
                meta_name.map_or("", String::as_str),
            ));
            if let Some(meta_name) = meta_name {
                manifests.push((dir.join(meta_name), entry.clone()));
            }
        }
        let stamp = format!("{}\n{}", names.len(), parts.join("\n"));
        if let Some((s, sessions)) = &*self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            if *s == stamp {
                return Ok(sessions.clone());
            }
        }

        let mut sessions: Vec<Session> = Vec::new();
        for (meta_path, fallback_id) in manifests {
            let raw = std::fs::read_to_string(&meta_path).unwrap_or_default();
            if let Some(session) = manifest_to_session(&raw, &fallback_id) {
                sessions.push(session);
            }
        }
        sessions.sort_by(|a, b| {
            b.last_activity_at
                .partial_cmp(&a.last_activity_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((stamp, sessions.clone()));
        Ok(sessions)
    }

    /// No native paging — load the session and slice its nodes, mirroring
    /// `DevinStore`'s window math (`before`/`limit` from the tail).
    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        let Some(session) = self.get_by_id(id, options.agent_id.as_deref()).await? else {
            return Ok(None);
        };
        let total = session.nodes.len();
        let before = options
            .before
            .map_or(total, |b| (b.max(0) as usize).min(total));
        let limit = options.limit.unwrap_or(total).max(1);
        let start = before.saturating_sub(limit);
        let nodes: Vec<MessageNode> = session.nodes[start..before].to_vec();
        let tool_call_nodes: Vec<MessageNode> = session
            .nodes
            .iter()
            .filter(|n| !n.tool_calls.is_empty())
            .cloned()
            .collect();
        Ok(Some(SessionNodeWindow {
            nodes,
            tool_call_nodes,
            total,
            start,
            backend_type: session.backend_type,
        }))
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        let dir = self.sessions_dir().join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(storage_err("Failed to delete cline session"))?;
        }
        let db_path = self.db_path();
        if db_path.exists() {
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                conn.execute("DELETE FROM sessions WHERE session_id = ?", params![id])
                    .ok();
            }
        }
        self.invalidate();
        Ok(())
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self.sessions_dir().join(id).exists())
    }
}

/// `session.rewind` — the control plane computed the cut; apply it to the
/// transcript the plan's nodes came from. The truncated IR is already
/// implied by the slice, so only `plan` is needed.
#[async_trait]
impl sepia_driver_sdk::SessionTruncator for ClineStore {
    async fn truncate(
        &self,
        session: &Session,
        plan: &sepia_core::rewind::RewindPlan,
        _truncated: &Session,
    ) -> Result<(), String> {
        self.truncate_session(&session.id, &plan.kept, &plan.removed)
            .map(|_| ())
            .map_err(|e| e.message)
    }
}
