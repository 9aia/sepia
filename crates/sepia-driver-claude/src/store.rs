//! `SessionRepository` over Claude Code's on-disk transcripts — port of
//! `packages/claude/src/ClaudeCodeRepository.ts`. The store has no manifest,
//! so `list()` reads each `.jsonl` and summarizes it without building nodes;
//! `get_by_id` parses the full transcript.
//!
//! Layout handled: `<projects>/<slug>/<uuid>.jsonl` main sessions plus
//! sub-agent transcripts in `<uuid>/subagents/agent-*.jsonl` (current) and
//! `agent-*.jsonl` at the project root (legacy).
//!
//! `save` writes the canonical layout: a top-level session lands at
//! `<projects>/<cwd-slug>/<id>.jsonl`; a session with `parent_session_id`
//! lands at `<slug>/<parent>/subagents/<id>.jsonl` (reusing the project dir
//! that already holds the parent when one exists). `delete` removes the
//! transcript plus its `<id>/` subagents dir.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sepia_core::domain::{MessageNode, Session, StorageError};
use sepia_core::rewind::RewindPlan;
use sepia_core::shared::{decode_project_dir, encode_project_dir};
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use serde_json::Value;

use crate::transcript::{ClaudeSourceInfo, from_file, summarize_jsonl, to_jsonl};

const JSONL: &str = ".jsonl";

/// A transcript file plus the provenance its path proves.
#[derive(Clone, Debug)]
pub struct TranscriptFile {
    pub file_path: PathBuf,
    pub id: String,
    pub fallback_cwd: String,
    pub parent_session_id: Option<String>,
}

fn storage_error(prefix: &str) -> impl Fn(std::io::Error) -> StorageError + '_ {
    move |cause| StorageError::new(format!("{prefix}: {cause}"))
}

/// Ids become path segments — no separators, NUL, or dot-dirs.
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// `fs.exists` — follows symlinks; any resolution failure answers `false`
/// (dangling links, ELOOP loops, unstatable paths).
fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Directory entry names; an unreadable dir degrades to empty (the
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

/// Every transcript file under the projects root, with its provenance.
/// A missing/unstatable root is an empty store; a root that exists but
/// cannot be listed is an error.
pub fn scan_transcript_files(projects_dir: &Path) -> Result<Vec<TranscriptFile>, std::io::Error> {
    if !exists(projects_dir) {
        return Ok(Vec::new());
    }
    let dirs = std::fs::read_dir(projects_dir)?;
    let mut files: Vec<TranscriptFile> = Vec::new();
    for dir_entry in dirs.flatten() {
        let dir = dir_entry.path();
        // `fs.stat` on the candidate — unstatable entries (dangling links)
        // and non-directories are skipped.
        let Ok(info) = std::fs::metadata(&dir) else {
            continue;
        };
        if !info.is_dir() {
            continue;
        }
        let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let fallback_cwd = decode_project_dir(dir_name);
        for entry in read_dir_names(&dir) {
            if let Some(id) = entry.strip_suffix(JSONL) {
                files.push(TranscriptFile {
                    file_path: dir.join(&entry),
                    id: id.to_string(),
                    fallback_cwd: fallback_cwd.clone(),
                    parent_session_id: None,
                });
                continue;
            }
            // A `<uuid>/` directory may hold `subagents/agent-*.jsonl`.
            let sub_dir = dir.join(&entry).join("subagents");
            if !exists(&sub_dir) {
                continue;
            }
            for sub in read_dir_names(&sub_dir) {
                let Some(id) = sub.strip_suffix(JSONL) else {
                    continue;
                };
                files.push(TranscriptFile {
                    file_path: sub_dir.join(&sub),
                    id: id.to_string(),
                    fallback_cwd: fallback_cwd.clone(),
                    parent_session_id: Some(entry.clone()),
                });
            }
        }
    }
    Ok(files)
}

fn file_mtime_ms(file_path: &Path) -> Option<f64> {
    std::fs::metadata(file_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
}

/// A `SessionRepository` over a Claude Code `projects/` directory.
pub struct ClaudeStore {
    projects_dir: PathBuf,
    /// Read cache — `list()` reads and summarizes every transcript on each
    /// call and a projects dir can hold hundreds of `.jsonl` files. The scan
    /// itself (dir walk + stats) is cheap, so the parsed array is keyed on a
    /// stamp of the file set: count + max mtime + every path with its mtime.
    /// Edits, adds and deletes all move the stamp; writes through this
    /// instance invalidate explicitly.
    list_cache: std::sync::Mutex<Option<(String, Vec<Session>)>>,
}

impl ClaudeStore {
    /// The store over `projects_dir` (usually `~/.claude/projects`).
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            list_cache: std::sync::Mutex::new(None),
        }
    }

    /// The store under a `.claude` dir — `~/.claude` maps to
    /// `~/.claude/projects`.
    pub fn for_claude_dir(claude_dir: &Path) -> Self {
        Self::new(claude_dir.join("projects"))
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    fn transcript_files(&self, prefix: &str) -> Result<Vec<TranscriptFile>, StorageError> {
        scan_transcript_files(&self.projects_dir).map_err(storage_error(prefix))
    }

    fn list_stamp(files: &[TranscriptFile]) -> String {
        let mut max_mtime = 0.0f64;
        let mut parts: Vec<String> = Vec::new();
        for file in files {
            let mtime = file_mtime_ms(&file.file_path);
            if let Some(mtime) = mtime {
                if mtime > max_mtime {
                    max_mtime = mtime;
                }
            }
            parts.push(format!(
                "{}:{}",
                file.file_path.display(),
                mtime.map_or_else(|| "?".into(), |m| m.to_string())
            ));
        }
        format!("{}:{}\n{}", files.len(), max_mtime, parts.join("\n"))
    }

    fn invalidate(&self) {
        *self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Rewind write — [`truncate_claude_transcript`] port.
    ///
    /// # Errors
    /// `StorageError` when the session is unknown, a kept node cannot be
    /// located in the transcript, or the entry order cannot honor the cut.
    pub fn truncate_transcript(
        &self,
        session_id: &str,
        kept: &[MessageNode],
        removed: &[MessageNode],
    ) -> Result<usize, StorageError> {
        truncate_claude_transcript(&self.projects_dir, session_id, kept, removed)
    }
}

#[async_trait]
impl SessionRepository for ClaudeStore {
    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let files = self.transcript_files("Failed to list claude sessions")?;
        let stamp = Self::list_stamp(&files);
        if let Some((cached_stamp, sessions)) = &*self
            .list_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            if *cached_stamp == stamp {
                return Ok(sessions.clone());
            }
        }
        let mut sessions: Vec<Session> = Vec::new();
        for file in &files {
            // Unreadable transcripts are skipped, not fatal.
            let raw = std::fs::read_to_string(&file.file_path).unwrap_or_default();
            if raw.is_empty() {
                continue;
            }
            sessions.push(summarize_jsonl(
                &raw,
                &ClaudeSourceInfo {
                    id: file.id.clone(),
                    fallback_cwd: Some(file.fallback_cwd.clone()),
                    parent_session_id: file.parent_session_id.clone(),
                },
            ));
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
        let files = scan_transcript_files(&self.projects_dir)
            .map_err(storage_error("Failed to read claude session"))?;
        let Some(file) = files.iter().find(|f| f.id == id) else {
            return Ok(None);
        };
        // `from_file` re-derives id, decoded cwd and subagent parentage from
        // the path — the same values the scan computed.
        match from_file(&file.file_path, None, None) {
            Ok(session) => Ok(Some(session)),
            // A transcript that vanished since the scan reads as absent;
            // other failures surface.
            Err(e) if e.message.to_lowercase().contains("not found") => Ok(None),
            Err(e) => Err(StorageError::new(format!(
                "Failed to read claude session: {}",
                e.message
            ))),
        }
    }

    async fn summary(
        &self,
        id: &str,
        _agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        // The summary shape is what `list()` serves: meta, checkpoints and
        // prompt history without the node backlog.
        let file = self
            .transcript_files("Failed to read claude session")?
            .into_iter()
            .find(|f| f.id == id);
        let Some(file) = file else {
            return Ok(None);
        };
        // A transcript that vanished since the scan reads as absent.
        let raw = match std::fs::read_to_string(&file.file_path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(storage_error("Failed to read claude session")(e)),
        };
        Ok(Some(summarize_jsonl(
            &raw,
            &ClaudeSourceInfo {
                id: file.id,
                fallback_cwd: Some(file.fallback_cwd),
                parent_session_id: file.parent_session_id,
            },
        )))
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        // A transcript is one file — parse once, then page in memory.
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
        let parent_id = session.parent_session_id.as_deref();
        for unsafe_name in [Some(session.id.as_str()), parent_id].into_iter().flatten() {
            if !is_safe_file_name(unsafe_name) {
                return Err(StorageError::new(format!(
                    "Claude session id is not a safe file name: {}",
                    serde_json::to_string(unsafe_name).unwrap_or_default()
                )));
            }
        }

        let file_path: PathBuf = if let Some(parent_id) = parent_id {
            // A subagent transcript lives at `<slug>/<parent>/subagents/` —
            // reuse the project that already holds the parent, else this
            // session's own project.
            let mut slug = encode_project_dir(&session.working_directory);
            for candidate in read_dir_names(&self.projects_dir) {
                if exists(
                    &self
                        .projects_dir
                        .join(&candidate)
                        .join(format!("{parent_id}{JSONL}")),
                ) {
                    slug = candidate;
                    break;
                }
            }
            let dir = self
                .projects_dir
                .join(&slug)
                .join(parent_id)
                .join("subagents");
            std::fs::create_dir_all(&dir)
                .map_err(storage_error("Failed to save claude session"))?;
            dir.join(format!("{}{JSONL}", session.id))
        } else {
            // Canonical layout — `<projects>/<cwd-slug>/<id>.jsonl`.
            let dir = self
                .projects_dir
                .join(encode_project_dir(&session.working_directory));
            std::fs::create_dir_all(&dir)
                .map_err(storage_error("Failed to save claude session"))?;
            dir.join(format!("{}{JSONL}", session.id))
        };
        std::fs::write(&file_path, to_jsonl(session))
            .map_err(storage_error("Failed to save claude session"))?;
        self.invalidate();
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        if !is_safe_file_name(id) {
            return Err(StorageError::new(format!(
                "Claude session id is not a safe file name: {}",
                serde_json::to_string(id).unwrap_or_default()
            )));
        }
        let file = self
            .transcript_files("Failed to delete claude session")?
            .into_iter()
            .find(|f| f.id == id);
        let Some(file) = file else {
            return Ok(());
        };
        std::fs::remove_file(&file.file_path)
            .map_err(storage_error("Failed to delete claude session"))?;
        // A main transcript's `<id>/` dir holds its subagents — remove it
        // with the session; a subagent file has no dir of its own.
        if let Some(parent) = file.file_path.parent() {
            let side_dir = parent.join(id);
            if exists(&side_dir) {
                std::fs::remove_dir_all(&side_dir)
                    .map_err(storage_error("Failed to delete claude session"))?;
            }
        }
        self.invalidate();
        Ok(())
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        Ok(self
            .transcript_files("Failed to check claude session")?
            .iter()
            .any(|f| f.id == id))
    }
}

#[async_trait]
impl sepia_driver_sdk::SessionTruncator for ClaudeStore {
    /// `session.rewind` — the prefix rewrite drops trailing entries; the
    /// `truncated` IR is informational only (the file itself is the state).
    async fn truncate(
        &self,
        session: &Session,
        plan: &RewindPlan,
        _truncated: &Session,
    ) -> Result<(), String> {
        self.truncate_transcript(&session.id, &plan.kept, &plan.removed)
            .map(|_| ())
            .map_err(|e| e.message)
    }
}

/// The transcript entry's `uuid` a reader-tagged node carries.
fn node_uuid(node: &MessageNode) -> Option<&str> {
    let uuid = node.metadata.get("uuid")?.as_str()?;
    (!uuid.is_empty()).then_some(uuid)
}

/// In-place conversation rewind for a Claude Code transcript: rewrite the
/// `.jsonl` so it ends with the entry that produced the last kept node.
///
/// The file is append-only by convention — entries can't be deleted in
/// place, so the truncation is a prefix rewrite: every node records its
/// source entry's `uuid` in `metadata.uuid`, the uuid→line map locates the
/// cut, and lines past it are dropped. Kept lines are preserved byte for
/// byte (no re-serialization), so the surviving transcript is identical to
/// what the agent wrote — including plumbing entries (`summary`,
/// `file-history-snapshot`, `queue-operation`) that emit no node but sit
/// before the cut.
///
/// One JSONL entry can emit several nodes (a `user` entry's `tool_result`
/// blocks become `tool` nodes); a node removed from an entry the cut keeps
/// is a boundary survivor — the entry can't be split. A removed node on an
/// *earlier* line than the cut means the file's order can't honor the
/// requested boundary and the truncation refuses instead of silently
/// over-keeping.
///
/// Returns the number of dropped transcript entries.
pub fn truncate_claude_transcript(
    projects_dir: &Path,
    session_id: &str,
    kept: &[MessageNode],
    removed: &[MessageNode],
) -> Result<usize, StorageError> {
    let prefix = "Failed to truncate claude session";
    let files = scan_transcript_files(projects_dir).map_err(storage_error(prefix))?;
    let Some(file) = files.iter().find(|f| f.id == session_id) else {
        return Err(StorageError::new(format!(
            "Claude transcript not found: {session_id}"
        )));
    };
    let raw = std::fs::read_to_string(&file.file_path).map_err(storage_error(prefix))?;
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    // uuid → line index; entries without one (or that don't parse) are
    // plumbing — they can't anchor a node either way.
    let mut line_by_uuid: HashMap<String, usize> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        if let Ok(entry) = serde_json::from_str::<Value>(line) {
            if let Some(uuid) = entry.get("uuid").and_then(Value::as_str) {
                line_by_uuid.insert(uuid.to_string(), index);
            }
        }
    }

    let line_of = |node: &MessageNode| -> Result<usize, StorageError> {
        let line = node_uuid(node).and_then(|uuid| line_by_uuid.get(uuid).copied());
        line.ok_or_else(|| {
            StorageError::new(format!(
                "Node {} has no locatable transcript entry — cannot truncate",
                node.node_id
            ))
        })
    };
    let mut kept_lines: Vec<usize> = Vec::with_capacity(kept.len());
    for node in kept {
        kept_lines.push(line_of(node)?);
    }
    let mut removed_lines: Vec<usize> = Vec::with_capacity(removed.len());
    for node in removed {
        removed_lines.push(line_of(node)?);
    }

    let kept_set: HashSet<usize> = kept_lines.iter().copied().collect();
    let cut_line = kept_lines.iter().max().map_or(0, |m| m + 1);
    for line in &removed_lines {
        if *line < cut_line && !kept_set.contains(line) {
            return Err(StorageError::new(format!(
                "The transcript's entry order cannot honor this cut — a removed node sits at line {}, inside the kept prefix",
                line + 1
            )));
        }
    }
    if cut_line >= lines.len() {
        return Ok(0);
    }

    let trailing_newline = if raw.ends_with('\n') { "\n" } else { "" };
    let mut tmp_name = file.file_path.as_os_str().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(
        &tmp,
        format!("{}{trailing_newline}", lines[..cut_line].join("\n")),
    )
    .map_err(storage_error(prefix))?;
    std::fs::rename(&tmp, &file.file_path).map_err(storage_error(prefix))?;
    Ok(lines.len() - cut_line)
}
