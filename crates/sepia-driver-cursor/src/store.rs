//! `CursorRepository.ts` port — the `SessionRepository` over Cursor's two
//! on-disk stores:
//!
//! - `chats/<workspace-hash>/<chat-id>/` — the agent CLI's content-addressed
//!   `store.db` plus `meta.json`/`prompt_history.json` sidecars. Canonical
//!   transcript: ordered JSON message blobs listed by the latest checkpoint.
//! - `projects/<slug>/agent-transcripts/<chat-id>/<chat-id>.jsonl` — the
//!   lossy text projection kept for every chat, including ones whose
//!   `store.db` was pruned. A chat covered by `chats/` wins on the shared id;
//!   `<chat-id>/subagents/<id>.jsonl` files link to their parent chat.
//!
//! Both trees degrade to empty when the dir is missing, matching the other
//! overlay repositories.
//!
//! `save` writes a top-level session canonically: the chat dir resolves to
//! `chats/<md5(cwd)>/<id>` (an existing chats dir for the id, under any
//! workspace hash, is updated in place), `store.db` gets the session's
//! message blobs plus a fresh checkpoint and meta root — blob inserts are
//! additive, so rewriting keeps the store's older DAG entries — and the
//! `meta.json`/`prompt_history.json` sidecars follow. The transcript
//! projection is written alongside, matching Cursor's own dual write.
//! Subagent sessions have no chats-store concept and stay transcript-only.
//! `delete` removes both the chat store dir and the transcript dir for the
//! id.
//!
//! One Rust-side addition the TS adapter doesn't have: `meta.json` carries a
//! `sepia` block with the full IR and the store root it was written against
//! (`{"sepia": {"storeRoot", "session"}}`). The store format is lossy —
//! diffs/locations/extra node fields can't be re-encoded — so `get_by_id`
//! prefers the embedded IR when the store's `latestRootBlobId` still matches
//! the recorded root (a Cursor-side append moves the root and the embed is
//! ignored). Mirrors the `sepia` transcript sidecar `sepia-driver-cline`
//! writes for the same reason.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags};
use sepia_core::domain::{MessageNode, Session, StorageError};
use sepia_core::shared::decode_project_dir;
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use serde_json::Value;

use crate::cursor::{
    self, CursorChatInfo, CursorStoreInput, CursorStoreMeta, CursorTranscriptSource,
    SummarizeStoreInput,
};

/// Embedded-IR key inside `meta.json` — `metaJson["sepia"]["session"]`
/// carries the wire payload `save` wrote, so `get_by_id` can splice the
/// fields the store format can't hold back into the native decode.
const SIDECAR_KEY: &str = "sepia";

const JSONL: &str = ".jsonl";

fn storage_error(prefix: &str) -> impl Fn(std::io::Error) -> StorageError + '_ {
    move |cause| StorageError::new(format!("{prefix}: {cause}"))
}

/// Ids and slugs become path segments (`<chat-id>/<chat-id>.jsonl`), so they
/// must be a single safe file name — no separators, NUL, or dot-dirs.
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// `fs.exists` — follows symlinks; any resolution failure answers `false`
/// (dangling links, ELOOP loops, unstatable paths).
fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Directory entry names; an unreadable/missing dir degrades to empty (the
/// `orElseSucceed([])` convention of the TS adapter).
fn read_dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

fn read_file_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// statSync mtime — a missing/unstatable file reads as `None`.
fn file_mtime_ms(file_path: &Path) -> Option<f64> {
    std::fs::metadata(file_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
}

fn read_meta_json(chat_dir: &Path, has_meta: bool) -> Option<Value> {
    if !has_meta {
        return None;
    }
    serde_json::from_str::<Value>(&read_file_or_empty(&chat_dir.join("meta.json"))).ok()
}

struct ChatEntry {
    dir: PathBuf,
    id: String,
    workspace_hash: String,
    has_store: bool,
    has_meta: bool,
}

struct TranscriptEntry {
    file_path: PathBuf,
    id: String,
    project_slug: String,
    parent_session_id: Option<String>,
}

enum StoreEntry {
    Chat(ChatEntry),
    Transcript(TranscriptEntry),
}

impl StoreEntry {
    fn id(&self) -> &str {
        match self {
            StoreEntry::Chat(e) => &e.id,
            StoreEntry::Transcript(e) => &e.id,
        }
    }
}

/// Every chat dir and transcript file under the cursor root, with
/// provenance. Missing roots scan as empty; unreadable dirs are skipped.
fn scan_entries(cursor_dir: &Path) -> Vec<StoreEntry> {
    let mut entries: Vec<StoreEntry> = Vec::new();

    let chats_dir = cursor_dir.join("chats");
    for ws_hash in read_dir_names(&chats_dir) {
        let ws_dir = chats_dir.join(&ws_hash);
        if !is_dir(&ws_dir) {
            continue;
        }
        for chat_id in read_dir_names(&ws_dir) {
            let chat_dir = ws_dir.join(&chat_id);
            if !is_dir(&chat_dir) {
                continue;
            }
            let files = read_dir_names(&chat_dir);
            entries.push(StoreEntry::Chat(ChatEntry {
                dir: chat_dir,
                id: chat_id,
                workspace_hash: ws_hash.clone(),
                has_store: files.iter().any(|f| f == "store.db"),
                has_meta: files.iter().any(|f| f == "meta.json"),
            }));
        }
    }

    let projects_dir = cursor_dir.join("projects");
    for slug in read_dir_names(&projects_dir) {
        let transcripts_dir = projects_dir.join(&slug).join("agent-transcripts");
        for chat_id in read_dir_names(&transcripts_dir) {
            let chat_dir = transcripts_dir.join(&chat_id);
            if !is_dir(&chat_dir) {
                continue;
            }
            let main = chat_dir.join(format!("{chat_id}{JSONL}"));
            if exists(&main) {
                entries.push(StoreEntry::Transcript(TranscriptEntry {
                    file_path: main,
                    id: chat_id.clone(),
                    project_slug: slug.clone(),
                    parent_session_id: None,
                }));
            }
            let sub_dir = chat_dir.join("subagents");
            for sub in read_dir_names(&sub_dir) {
                let Some(id) = sub.strip_suffix(JSONL) else {
                    continue;
                };
                entries.push(StoreEntry::Transcript(TranscriptEntry {
                    file_path: sub_dir.join(&sub),
                    id: id.to_string(),
                    project_slug: slug.clone(),
                    parent_session_id: Some(chat_id.clone()),
                }));
            }
        }
    }
    entries
}

/* ------------------------------------------------------------------ */
/* store.db queries — `all` that degrades a failure to []              */
/* ------------------------------------------------------------------ */

/// `bun:sqlite` read-only open — a `0`-byte or schema-less `store.db` opens
/// fine but has no tables; query failures degrade per query, never at open.
fn open_store(store_path: &Path) -> Result<Connection, StorageError> {
    Connection::open_with_flags(
        store_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        StorageError::new(format!(
            "Failed to open cursor store {}: {e}",
            store_path.display()
        ))
    })
}

/// `meta['0']` — the TEXT value only; other storage classes read as absent
/// (`parseStoreMeta` rejects non-strings the same way).
fn meta_row(conn: &Connection) -> Option<String> {
    conn.query_row("select value from meta where key = '0'", [], |r| {
        r.get::<_, rusqlite::types::Value>(0)
    })
    .ok()
    .and_then(|v| match v {
        rusqlite::types::Value::Text(s) => Some(s),
        _ => None,
    })
}

fn blob_row(conn: &Connection, id: &str) -> Option<Vec<u8>> {
    conn.query_row("select data from blobs where id = ?", [id], |r| {
        r.get::<_, rusqlite::types::Value>(0)
    })
    .ok()
    .and_then(|v| match v {
        rusqlite::types::Value::Blob(data) => Some(data),
        _ => None,
    })
}

fn all_blobs(conn: &Connection) -> Vec<(String, Vec<u8>)> {
    let Ok(mut stmt) = conn.prepare("select id, data from blobs") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, rusqlite::types::Value>(0)?,
            r.get::<_, rusqlite::types::Value>(1)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|(id, data)| match (id, data) {
            (rusqlite::types::Value::Text(id), rusqlite::types::Value::Blob(data)) => {
                Some((id, data))
            }
            _ => None,
        })
        .collect()
}

/// `meta['0']` plus the root checkpoint's workspace, from an open store.
fn store_summary(chat_dir: &Path) -> (Option<CursorStoreMeta>, Option<String>) {
    let Ok(conn) = open_store(&chat_dir.join("store.db")) else {
        return (None, None);
    };
    let meta = meta_row(&conn).and_then(|v| cursor::parse_store_meta(&v));
    let mut workspace: Option<String> = None;
    if let Some(root_id) = meta
        .as_ref()
        .and_then(|m| m.latest_root_blob_id.as_deref())
        .filter(|id| !id.is_empty())
    {
        if let Some(data) = blob_row(&conn, root_id) {
            workspace = cursor::workspace_from_uri(
                cursor::decode_checkpoint(&data)
                    .and_then(|c| c.workspace)
                    .as_deref(),
            );
        }
    }
    (meta, workspace)
}

/**
 * `meta['0']` of an existing store for a `save` rewrite — `None` whenever
 * the store is missing, empty or unopenable (a fresh chat dir has no meta
 * row yet).
 */
fn store_meta_row(store_path: &Path) -> Option<CursorStoreMeta> {
    let conn = open_store(store_path).ok()?;
    meta_row(&conn).and_then(|v| cursor::parse_store_meta(&v))
}

fn chat_summary(entry: &ChatEntry, fallback_cwd: Option<&str>) -> Session {
    let meta_json = read_meta_json(&entry.dir, entry.has_meta)
        .as_ref()
        .and_then(cursor::parse_meta_json);
    if !entry.has_store {
        return cursor::summarize_store(&SummarizeStoreInput {
            chat: CursorChatInfo {
                id: entry.id.clone(),
                workspace_hash: Some(entry.workspace_hash.clone()),
                fallback_cwd: fallback_cwd.map(str::to_string),
            },
            meta_json,
            mtime_ms: file_mtime_ms(&entry.dir.join("meta.json")),
            ..Default::default()
        });
    }
    let (meta, workspace) = store_summary(&entry.dir);
    cursor::summarize_store(&SummarizeStoreInput {
        chat: CursorChatInfo {
            id: entry.id.clone(),
            workspace_hash: Some(entry.workspace_hash.clone()),
            fallback_cwd: fallback_cwd.map(str::to_string),
        },
        meta,
        meta_json,
        workspace,
        mtime_ms: file_mtime_ms(&entry.dir.join("store.db")),
    })
}

/// Would `node` emit a message blob through `message_json`? Used to align
/// an embedded IR's nodes with the nodes the native decode produced —
/// empty `system`/`user` content and empty `assistant` turns emit nothing.
fn node_emits_blob(node: &MessageNode) -> bool {
    use sepia_core::domain::Role;
    match node.role {
        Role::System | Role::User => !node.content.is_empty(),
        Role::Assistant => {
            node.thinking.is_some() || !node.content.is_empty() || !node.tool_calls.is_empty()
        }
        Role::Tool => true,
    }
}

/// Metadata keys the native decode knows and the embed can't: the blob id
/// a node was read from, the paired tool args, the `<user_info>` marker.
fn overlay_metadata(embedded: &Value, native: &Value) -> Value {
    let mut merged = match native.as_object() {
        Some(obj) => obj.clone(),
        None => return embedded.clone(),
    };
    if let Some(extra) = embedded.as_object() {
        for (k, v) in extra {
            merged.insert(k.clone(), v.clone());
        }
    }
    Value::Object(merged)
}

/**
 * Splice the saved IR back onto the native decode: session fields and
 * metadata stay native (they reflect the store's real state), nodes come
 * from the embed (carrying diffs/locations/thinking/usage the blob format
 * cannot hold), with per-node `metadata` overlaid so `blobId`/
 * `toolArguments`/`context` survive for a later rewrite. Emitting nodes
 * align with native nodes 1:1 in order (a grouped tool blob still emits
 * one native node per embedded tool node).
 */
fn merge_embedded(native: &Session, embedded: &Session) -> Session {
    let native_by_emit: Vec<&MessageNode> = native.nodes.iter().collect();
    let mut native_iter = native_by_emit.into_iter();
    let nodes: Vec<MessageNode> = embedded
        .nodes
        .iter()
        .map(|node| {
            let mut node = node.clone();
            if node_emits_blob(&node) {
                if let Some(native_node) = native_iter.next() {
                    if native_node.role == node.role {
                        node.metadata = overlay_metadata(&node.metadata, &native_node.metadata);
                    }
                }
            }
            node
        })
        .collect();
    let mut session = native.clone();
    session.nodes = nodes;
    if !embedded.prompt_history.is_empty() {
        session.prompt_history.clone_from(&embedded.prompt_history);
    }
    session
}

fn transcript_session(entry: &TranscriptEntry, full: bool) -> Session {
    let raw = read_file_or_empty(&entry.file_path);
    let source = CursorTranscriptSource {
        id: entry.id.clone(),
        project_slug: entry.project_slug.clone(),
        parent_session_id: entry.parent_session_id.clone(),
        mtime_ms: file_mtime_ms(&entry.file_path),
    };
    if full {
        cursor::from_transcript_jsonl(&raw, &source)
    } else {
        cursor::summarize_transcript_jsonl(&raw, &source)
    }
}

/// A `SessionRepository` over a `.cursor` dir (`chats/` + `projects/`).
pub struct CursorStore {
    cursor_dir: PathBuf,
    /// Read cache — `list()` opens every chat's `store.db` and reads every
    /// transcript on each call. The entry scan is the cheap half, so the
    /// summarized array is keyed on a stamp built from it: each chat dir's
    /// `store.db` (+ its WAL — external writes may touch only that, like the
    /// devin store) and sidecar mtimes, each transcript's mtime, and the
    /// entry set itself. Writes through this instance invalidate explicitly.
    list_cache: Mutex<Option<(String, Vec<Session>)>>,
}

impl CursorStore {
    /// The store under a `.cursor` dir — usually `~/.cursor`.
    pub fn new(cursor_dir: PathBuf) -> Self {
        Self {
            cursor_dir,
            list_cache: Mutex::new(None),
        }
    }

    pub fn cursor_dir(&self) -> &Path {
        &self.cursor_dir
    }

    fn chats_dir(&self) -> PathBuf {
        self.cursor_dir.join("chats")
    }

    fn projects_dir(&self) -> PathBuf {
        self.cursor_dir.join("projects")
    }

    fn list_stamp(entries: &[StoreEntry]) -> String {
        let mut max_mtime = 0.0f64;
        let mut parts: Vec<String> = Vec::new();
        let mut bump = |path: PathBuf| {
            let mtime = file_mtime_ms(&path);
            if let Some(m) = mtime {
                if m > max_mtime {
                    max_mtime = m;
                }
            }
            format!(
                "{}:{}",
                path.display(),
                mtime.map_or_else(|| "?".into(), |m| m.to_string())
            )
        };
        for entry in entries {
            match entry {
                StoreEntry::Chat(e) => {
                    parts.push(format!(
                        "{}:{}:{}",
                        e.dir.display(),
                        e.has_store,
                        e.has_meta
                    ));
                    parts.push(bump(e.dir.join("store.db")));
                    parts.push(bump(e.dir.join("store.db-wal")));
                    parts.push(bump(e.dir.join("meta.json")));
                    parts.push(bump(e.dir.join("prompt_history.json")));
                }
                StoreEntry::Transcript(e) => parts.push(bump(e.file_path.clone())),
            }
        }
        format!("{}:{}\n{}", entries.len(), max_mtime, parts.join("\n"))
    }

    fn invalidate(&self) {
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn chat_session(
        entry: &ChatEntry,
        fallback_cwd: Option<&str>,
    ) -> Result<Session, StorageError> {
        let meta_json_value = read_meta_json(&entry.dir, entry.has_meta);
        let meta_json = meta_json_value.as_ref().and_then(cursor::parse_meta_json);
        if !entry.has_store {
            return Ok(cursor::summarize_store(&SummarizeStoreInput {
                chat: CursorChatInfo {
                    id: entry.id.clone(),
                    workspace_hash: Some(entry.workspace_hash.clone()),
                    fallback_cwd: fallback_cwd.map(str::to_string),
                },
                meta_json,
                mtime_ms: file_mtime_ms(&entry.dir.join("meta.json")),
                ..Default::default()
            }));
        }
        let conn = open_store(&entry.dir.join("store.db"))?;
        let meta = meta_row(&conn).and_then(|v| cursor::parse_store_meta(&v));
        let blobs = all_blobs(&conn);
        let prompt_history = cursor::parse_prompt_history(&read_file_or_empty(
            &entry.dir.join("prompt_history.json"),
        ));
        let native = cursor::session_from_store(&CursorStoreInput {
            id: entry.id.clone(),
            workspace_hash: Some(entry.workspace_hash.clone()),
            fallback_cwd: fallback_cwd.map(str::to_string),
            meta: meta.clone(),
            meta_json,
            blobs,
            prompt_history,
        });
        drop(conn);

        // The embedded `sepia` IR is trusted only while the store root it
        // was written against still matches `latestRootBlobId` — a Cursor-
        // side append moves the root and the embed is ignored.
        let embedded = meta_json_value
            .as_ref()
            .and_then(|v| v.get(SIDECAR_KEY))
            .and_then(|s| s.get("session"))
            .and_then(|s| sepia_core::wire::session_from_json(s).ok());
        let Some(embedded) = embedded else {
            return Ok(native);
        };
        let recorded_root = meta_json_value
            .as_ref()
            .and_then(|v| v.get(SIDECAR_KEY))
            .and_then(|s| s.get("storeRoot"))
            .and_then(Value::as_str);
        let current_root = meta.as_ref().and_then(|m| m.latest_root_blob_id.as_deref());
        if recorded_root != current_root || recorded_root.is_none() {
            return Ok(native);
        }
        Ok(merge_embedded(&native, &embedded))
    }
}

#[async_trait]
impl SessionRepository for CursorStore {
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let entries = scan_entries(&self.cursor_dir);
        let stamp = Self::list_stamp(&entries);
        if let Some((cached_stamp, sessions)) = &*self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            if *cached_stamp == stamp {
                return Ok(sessions.clone());
            }
        }
        let chat_ids: HashSet<&str> = entries
            .iter()
            .filter_map(|e| match e {
                StoreEntry::Chat(c) => Some(c.id.as_str()),
                StoreEntry::Transcript(_) => None,
            })
            .collect();
        // A transcript's project slug decodes to a real cwd — use it as the
        // fallback for chats whose store doesn't record one (the workspace
        // hash dir name is opaque).
        let mut transcript_cwd: HashMap<&str, String> = HashMap::new();
        for entry in &entries {
            if let StoreEntry::Transcript(e) = entry {
                if e.parent_session_id.is_none() {
                    transcript_cwd.insert(&e.id, decode_project_dir(&e.project_slug));
                }
            }
        }
        let mut sessions: Vec<Session> = Vec::new();
        for entry in &entries {
            // A chat store.db wins over the lossy transcript of the same chat.
            if let StoreEntry::Transcript(e) = entry {
                if chat_ids.contains(e.id.as_str()) {
                    continue;
                }
            }
            let session = match entry {
                StoreEntry::Chat(e) => {
                    chat_summary(e, transcript_cwd.get(e.id.as_str()).map(String::as_str))
                }
                StoreEntry::Transcript(e) => transcript_session(e, false),
            };
            sessions.push(session);
        }
        sessions.sort_by(|a, b| b.last_activity_at.total_cmp(&a.last_activity_at));
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((stamp, sessions.clone()));
        Ok(sessions)
    }

    async fn get_by_id(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        let entries = scan_entries(&self.cursor_dir);
        let chat_ids: HashSet<&str> = entries
            .iter()
            .filter_map(|e| match e {
                StoreEntry::Chat(c) => Some(c.id.as_str()),
                StoreEntry::Transcript(_) => None,
            })
            .collect();
        for entry in &entries {
            if entry.id() != id {
                continue;
            }
            match entry {
                StoreEntry::Chat(e) => {
                    let fallback = entries.iter().find(|candidate| {
                        matches!(candidate, StoreEntry::Transcript(t)
                            if t.id == id && t.parent_session_id.is_none())
                    });
                    let fallback_cwd = fallback.and_then(|candidate| match candidate {
                        StoreEntry::Transcript(t) => Some(decode_project_dir(&t.project_slug)),
                        StoreEntry::Chat(_) => None,
                    });
                    let session = Self::chat_session(e, fallback_cwd.as_deref())?;
                    // The blob store may be pruned to nothing while the
                    // transcript projection still holds the conversation —
                    // prefer whichever decoding carries more nodes; keep the
                    // store's richer meta (title, prompt history) when both
                    // are empty.
                    if let Some(StoreEntry::Transcript(t)) = fallback {
                        let projected = transcript_session(t, true);
                        if projected.nodes.len() > session.nodes.len() {
                            return Ok(Some(projected));
                        }
                    }
                    return Ok(Some(session));
                }
                StoreEntry::Transcript(e) => {
                    if chat_ids.contains(id) {
                        continue;
                    }
                    return Ok(Some(transcript_session(e, true)));
                }
            }
        }
        Ok(None)
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        // A chat store is one sqlite + a transcript is one file — read
        // once, then page in memory.
        let Some(session) = self.get_by_id(id, options.agent_id.as_deref()).await? else {
            return Ok(None);
        };
        let total = session.nodes.len();
        let before = options.before.map_or(total, |b| {
            usize::try_from(b.max(0)).unwrap_or(usize::MAX).min(total)
        });
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

    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        let prefix = "Failed to save cursor session";
        let parent_id = session.parent_session_id.as_deref();
        for unsafe_name in [Some(session.id.as_str()), parent_id].into_iter().flatten() {
            if !is_safe_file_name(unsafe_name) {
                return Err(StorageError::new(format!(
                    "Cursor session id is not a safe file name: {}",
                    serde_json::to_string(unsafe_name).unwrap_or_default()
                )));
            }
        }

        if parent_id.is_none() {
            // Canonical write — the chats store.db. An existing chats dir
            // for the id (under any workspace hash) is updated in place;
            // otherwise the dir is `chats/<md5(cwd)>/<id>`, the same mapping
            // Cursor itself uses.
            let mut chat_dir: Option<PathBuf> = None;
            for ws_hash in read_dir_names(&self.chats_dir()) {
                let candidate = self.chats_dir().join(&ws_hash).join(&session.id);
                if is_dir(&candidate) {
                    chat_dir = Some(candidate);
                    break;
                }
            }
            let chat_dir = chat_dir.unwrap_or_else(|| {
                self.chats_dir()
                    .join(cursor::workspace_hash_from_cwd(&session.working_directory))
                    .join(&session.id)
            });
            std::fs::create_dir_all(&chat_dir).map_err(storage_error(prefix))?;

            let store_path = chat_dir.join("store.db");
            // A rewrite keeps the chat's original creation stamp — the
            // meta.json sidecar records it, the meta row when the sidecar
            // was never written.
            let prior_meta = std::fs::read_to_string(chat_dir.join("meta.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|v| cursor::parse_meta_json(&v));
            let prior_store = store_meta_row(&store_path);
            let plan = cursor::store_write_plan(
                session,
                prior_meta
                    .and_then(|m| m.created_at_ms)
                    .or_else(|| prior_store.and_then(|m| m.created_at)),
            );

            let conn = Connection::open(&store_path).map_err(|e| {
                StorageError::new(format!(
                    "Failed to open cursor store {} for writing: {e}",
                    store_path.display()
                ))
            })?;
            (|| -> Result<(), rusqlite::Error> {
                // the same pragmas + schema the agent's own
                // `initializeDriver` runs (user_version 1, WAL)
                conn.execute_batch(
                    "PRAGMA journal_mode = WAL;
                     PRAGMA synchronous = NORMAL;
                     PRAGMA busy_timeout = 5000;
                     PRAGMA user_version = 1;
                     CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB);
                     CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
                )?;
                for blob in &plan.blobs {
                    conn.execute(
                        "INSERT OR REPLACE INTO blobs (id, data) VALUES (?, ?)",
                        rusqlite::params![blob.id, blob.data],
                    )?;
                }
                conn.execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES ('0', ?)",
                    [&plan.meta_row],
                )?;
                Ok(())
            })()
            .map_err(|e| {
                StorageError::new(format!(
                    "Failed to write cursor store {}: {e}",
                    store_path.display()
                ))
            })?;

            // meta.json = the canonical sidecar plus the embedded IR, so a
            // `get_by_id` can splice back fields the blob format can't hold.
            let mut meta_json: serde_json::Map<String, Value> =
                serde_json::from_str(&plan.meta_json).unwrap_or_default();
            let sidecar = sepia_core::wire::session_to_json(session).unwrap_or_default();
            meta_json.insert(
                SIDECAR_KEY.into(),
                serde_json::json!({
                    "storeRoot": plan.root_blob_id,
                    "session": sidecar,
                }),
            );
            std::fs::write(
                chat_dir.join("meta.json"),
                serde_json::to_string(&Value::Object(meta_json)).unwrap_or_default(),
            )
            .map_err(storage_error(prefix))?;
            if let Some(prompt_history) = &plan.prompt_history_json {
                std::fs::write(chat_dir.join("prompt_history.json"), prompt_history)
                    .map_err(storage_error(prefix))?;
            }
        }

        // Reuse the project slug a transcript read recorded; otherwise
        // derive it from the working directory.
        let meta = session.metadata.as_object();
        let recorded = meta
            .filter(|m| m.get("store").and_then(Value::as_str) == Some("transcript"))
            .and_then(|m| m.get("project").and_then(Value::as_str));
        let slug = match recorded.filter(|r| is_safe_file_name(r)) {
            Some(recorded) => recorded.to_string(),
            None => cursor::project_slug_from_cwd(&session.working_directory),
        };

        let file_path: PathBuf = if parent_id.is_none() {
            let dir = self
                .projects_dir()
                .join(&slug)
                .join("agent-transcripts")
                .join(&session.id);
            std::fs::create_dir_all(&dir).map_err(storage_error(prefix))?;
            dir.join(format!("{}{JSONL}", session.id))
        } else {
            // A subagent transcript lives under its parent chat's dir — find
            // the project that already holds the parent, else fall back to
            // this session's own project.
            let parent_id = parent_id.unwrap_or_default();
            let mut parent_slug = slug;
            for candidate in read_dir_names(&self.projects_dir()) {
                let parent_dir = self
                    .projects_dir()
                    .join(&candidate)
                    .join("agent-transcripts")
                    .join(parent_id);
                if exists(&parent_dir) {
                    parent_slug = candidate;
                    break;
                }
            }
            let dir = self
                .projects_dir()
                .join(&parent_slug)
                .join("agent-transcripts")
                .join(parent_id)
                .join("subagents");
            std::fs::create_dir_all(&dir).map_err(storage_error(prefix))?;
            dir.join(format!("{}{JSONL}", session.id))
        };

        std::fs::write(&file_path, cursor::to_transcript_jsonl(session))
            .map_err(storage_error(prefix))?;
        // The projection has no timestamps; the reader falls back to the
        // file mtime, so stamp it with the session's last activity. A
        // failed stamp degrades timestamps to "now", never the write.
        if let Ok(file) = std::fs::File::options().write(true).open(&file_path) {
            let mtime =
                UNIX_EPOCH + std::time::Duration::from_secs_f64(session.last_activity_at.max(0.0));
            let _ = file.set_modified(mtime);
        }
        self.invalidate();
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        if !is_safe_file_name(id) {
            return Err(StorageError::new(format!(
                "Cursor session id is not a safe file name: {}",
                serde_json::to_string(id).unwrap_or_default()
            )));
        }
        // `fs.rm(path, {recursive})` removes files and dirs alike.
        let remove_if_exists = |target: &Path, recursive: bool| -> Result<(), StorageError> {
            let Ok(meta) = std::fs::symlink_metadata(target) else {
                return Ok(());
            };
            if meta.is_dir() && recursive {
                std::fs::remove_dir_all(target)
            } else {
                std::fs::remove_file(target)
            }
            .map_err(storage_error("Failed to delete cursor session"))
        };

        for ws_hash in read_dir_names(&self.chats_dir()) {
            remove_if_exists(&self.chats_dir().join(&ws_hash).join(id), true)?;
        }
        for slug in read_dir_names(&self.projects_dir()) {
            let transcripts_dir = self.projects_dir().join(&slug).join("agent-transcripts");
            // The chat's own transcript dir (main file + its subagents)…
            remove_if_exists(&transcripts_dir.join(id), true)?;
            // …and a subagent file of the same id under another chat.
            for chat_id in read_dir_names(&transcripts_dir) {
                remove_if_exists(
                    &transcripts_dir
                        .join(&chat_id)
                        .join("subagents")
                        .join(format!("{id}{JSONL}")),
                    false,
                )?;
            }
        }
        self.invalidate();
        Ok(())
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(scan_entries(&self.cursor_dir).iter().any(|e| e.id() == id))
    }
}
