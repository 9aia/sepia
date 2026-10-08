//! Restore planning — pure math over a session's recorded `ToolCall::diffs`,
//! separated from filesystem/git side effects so the same code is unit
//! testable and the control plane can inject its own exec seam.
//!
//! Semantics: a recorded diff is `{path, old_text?, new_text?}` — the call
//! replaced `old_text` with `new_text` (absent `old_text` = the call created
//! the content, absent `new_text` = it removed it). The revert rule is
//! uniform: a step applies only when the recorded after-state is still
//! present verbatim, so a restore never clobbers drift.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::domain::{Session, ToolCallDiff};

/// A diff folded into a restore, tagged with the call that produced it.
pub struct RecordedDiff<'a> {
    pub tool_call_id: &'a str,
    pub diff: &'a ToolCallDiff,
}

/// What a restore would do to one path — computed before any write so the
/// caller can report `skip`/`unchanged` without touching the filesystem.
#[derive(Clone, Debug, PartialEq)]
pub enum FileRestorePlan {
    Write { path: String, content: String },
    Delete { path: String },
    Unchanged { path: String },
    Skip { path: String, reason: String },
}

/// Resolve `path` against `cwd`; `None` when the result escapes the
/// directory — `../` is never trusted. Mirrors `node:path.resolve`
/// semantics on POSIX (the only targets we support).
pub fn resolve_workspace_path(cwd: &str, path: &str) -> Option<PathBuf> {
    if path.trim().is_empty() {
        return None;
    }
    let root = normalize(Path::new(cwd));
    let candidate = Path::new(path);
    let resolved = if candidate.is_absolute() {
        normalize(candidate)
    } else {
        normalize(&root.join(candidate))
    };
    (resolved == root || resolved.starts_with(&root)).then_some(resolved)
}

/// `node:path.resolve`-style normalization without hitting the filesystem:
/// collapses `.`/`..`/duplicate separators lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn same_path(cwd: &str, a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (
        resolve_workspace_path(cwd, a),
        resolve_workspace_path(cwd, b),
    ) {
        (Some(ra), Some(rb)) => ra == rb,
        _ => false,
    }
}

/// Every recorded diff for `path`, in session order (earliest first).
/// Stores spell paths inconsistently, so matching goes through
/// cwd-normalized comparison with an exact string fast path.
pub fn diffs_for_path<'a>(session: &'a Session, path: &str) -> Vec<RecordedDiff<'a>> {
    let mut out = Vec::new();
    for node in &session.nodes {
        for call in &node.tool_calls {
            for diff in &call.diffs {
                if same_path(&session.working_directory, &diff.path, path) {
                    out.push(RecordedDiff {
                        tool_call_id: &call.id,
                        diff,
                    });
                }
            }
        }
    }
    out
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

enum RevertStep {
    Content(Option<String>),
    Skip(String),
}

/// Reverse-apply one recorded diff to `content` (`None` = file absent).
/// Any ambiguity is a skip, never a guess.
fn revert_step(content: Option<&str>, diff: &ToolCallDiff) -> RevertStep {
    let (old_text, new_text) = (&diff.old_text, &diff.new_text);
    match new_text {
        None => {
            // The call removed content — a file delete when `old_text` was
            // the whole file, a hunk removal otherwise. The only safe
            // revert is resurrecting `old_text` when the file is now absent.
            let Some(old_text) = old_text else {
                return RevertStep::Skip("the recorded diff carries no content".into());
            };
            if content.is_none() {
                return RevertStep::Content(Some(old_text.clone()));
            }
            RevertStep::Skip("cannot re-locate where the recorded removal happened".into())
        }
        Some(new_text) => match content {
            None => RevertStep::Content(old_text.clone()),
            Some(content) => {
                if content == new_text {
                    return RevertStep::Content(old_text.clone());
                }
                if new_text.is_empty() {
                    return RevertStep::Skip(
                        "the recorded after-state is empty and the file has since changed".into(),
                    );
                }
                match count_occurrences(content, new_text) {
                    0 => {
                        RevertStep::Skip("file no longer contains the recorded after-state".into())
                    }
                    1 => RevertStep::Content(Some(content.replacen(
                        new_text,
                        old_text.as_deref().unwrap_or(""),
                        1,
                    ))),
                    _ => RevertStep::Skip(
                        "the recorded after-state matches multiple positions".into(),
                    ),
                }
            }
        },
    }
}

/// Plan a file restore from recorded diffs.
///
/// With `tool_call_id`, reverts exactly that call's change to `path`.
/// Without it, folds every recorded diff for the path in reverse — the
/// result is the file's state before the session first touched it — and
/// any step that can't reverse-apply cleanly skips the whole restore.
pub fn plan_path_restore(
    session: &Session,
    path: &str,
    current: Option<&str>,
    tool_call_id: Option<&str>,
) -> FileRestorePlan {
    let recorded: Vec<RecordedDiff> = diffs_for_path(session, path)
        .into_iter()
        .filter(|entry| tool_call_id.is_none_or(|id| entry.tool_call_id == id))
        .collect();
    if recorded.is_empty() {
        return FileRestorePlan::Skip {
            path: path.into(),
            reason: match tool_call_id {
                None => "no recorded change for this path".into(),
                Some(id) => format!("no recorded change for this path in call {id}"),
            },
        };
    }
    let mut content: Option<String> = current.map(str::to_string);
    for entry in recorded.iter().rev() {
        match revert_step(content.as_deref(), entry.diff) {
            RevertStep::Content(next) => content = next,
            RevertStep::Skip(reason) => {
                return FileRestorePlan::Skip {
                    path: path.into(),
                    reason,
                };
            }
        }
    }
    match content {
        None => {
            if current.is_none() {
                FileRestorePlan::Unchanged { path: path.into() }
            } else {
                FileRestorePlan::Delete { path: path.into() }
            }
        }
        Some(content) => {
            if Some(content.as_str()) == current {
                FileRestorePlan::Unchanged { path: path.into() }
            } else {
                FileRestorePlan::Write {
                    path: path.into(),
                    content,
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* file-history checkpoints (Claude)                                   */
/* ------------------------------------------------------------------ */

/// `CheckpointRef::kind` a Claude `file-history-snapshot` entry maps to.
pub const FILE_HISTORY_KIND: &str = "file-history-snapshot";

/// One tracked file's backup pointer: the blob name under
/// `file-history/<sessionId>/`, or `None` for the deletion tombstone.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHistoryBackup {
    pub backup: Option<String>,
    pub version: Option<f64>,
}

/// A `metadata.fileHistory.snapshots[ref]` entry decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHistorySnapshot {
    pub session_id: String,
    pub files: BTreeMap<String, FileHistoryBackup>,
}

/// Tolerant read of `session.metadata.fileHistory` for one checkpoint ref.
/// `None` when the store recorded no map.
pub fn file_history_snapshot(session: &Session, ref_: &str) -> Option<FileHistorySnapshot> {
    let history = session.metadata.get("fileHistory")?;
    let session_id = history.get("sessionId")?.as_str()?.to_string();
    let entry = history.get("snapshots")?.get(ref_)?;
    let raw_files = entry.get("files")?.as_object()?;
    let mut files = BTreeMap::new();
    for (path, value) in raw_files {
        let Some(obj) = value.as_object() else {
            continue;
        };
        let backup = obj.get("backup")?;
        let backup = if backup.is_null() {
            None
        } else {
            match backup.as_str() {
                Some(s) => Some(s.to_string()),
                None => continue,
            }
        };
        let version = obj
            .get("version")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite());
        files.insert(path.clone(), FileHistoryBackup { backup, version });
    }
    Some(FileHistorySnapshot { session_id, files })
}
