//! sepia-meta — Sepia-owned per-session metadata (title overrides, pins,
//! project assignments, run spans) plus the user-defined project list and
//! server-persisted app config. Lives in a separate JSON file because the
//! agent session stores are opened read-only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock_inner(mutex: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One run span: which agent on which Sepia node continued a session.
/// Duplicated from `sepia_control::RunSpan` deliberately — the overlay is
/// node-local and must not pull the control crate in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSpan {
    /// Epoch milliseconds when the span was recorded (attach time).
    pub at: f64,
    pub agent: String,
    pub node: String,
}

/// Structural guard for a run span off the wire (transfer bundles carry
/// them).
pub fn is_run_span(value: &Value) -> bool {
    value.as_object().is_some_and(|raw| {
        let non_empty_str = |key: &str| {
            raw.get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        };
        raw.get("at")
            .and_then(Value::as_f64)
            .is_some_and(f64::is_finite)
            && non_empty_str("agent")
            && non_empty_str("node")
    })
}

/// Idempotent span append — a re-attach under the same agent+node is the
/// same run continuing, not a new one.
pub fn append_span(spans: &[RunSpan], span: RunSpan) -> Vec<RunSpan> {
    let mut next = spans.to_vec();
    match next.last() {
        Some(last) if last.agent == span.agent && last.node == span.node => next,
        _ => {
            next.push(span);
            next
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<String>>,
    /// Preferred spawn model for this session — applied on next attach.
    /// `Some(None)` on the wire = explicit clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Option<String>>,
    /// Run provenance: each attach appends a span recording which agent
    /// ran the session on which node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spans: Option<Vec<RunSpan>>,
    /// Recorded on `POST /api/sessions`: a created session may not exist
    /// in the agent's store yet — agent+cwd let the node identify it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl SessionMeta {
    /// Merge a wire patch (absent fields keep the current value).
    pub fn apply(&mut self, patch: &SessionMeta) {
        if patch.title.is_some() {
            self.title.clone_from(&patch.title);
        }
        if patch.pinned.is_some() {
            self.pinned = patch.pinned;
        }
        if patch.archived.is_some() {
            self.archived = patch.archived;
        }
        if patch.project_ids.is_some() {
            self.project_ids.clone_from(&patch.project_ids);
        }
        if patch.model.is_some() {
            self.model.clone_from(&patch.model);
        }
        if patch.spans.is_some() {
            self.spans.clone_from(&patch.spans);
        }
        if patch.agent.is_some() {
            self.agent.clone_from(&patch.agent);
        }
        if patch.cwd.is_some() {
            self.cwd.clone_from(&patch.cwd);
        }
        if patch.created_at.is_some() {
            self.created_at.clone_from(&patch.created_at);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Default, Serialize, Deserialize)]
struct MetaFile {
    #[serde(default)]
    sessions: BTreeMap<String, SessionMeta>,
    #[serde(default)]
    projects: BTreeMap<String, ProjectName>,
    /// Server-persisted app/UI config (collapse state, prefs).
    #[serde(default)]
    config: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectName {
    name: String,
}

/// Tolerant decode of a stored `SessionMeta` — migrates older shapes:
/// singular `projectId`, non-bool pins, junk fields dropped.
fn normalize_session(value: &Value) -> SessionMeta {
    let raw = value.as_object().cloned().unwrap_or_default();
    let project_ids = match &raw.get("projectIds").cloned().unwrap_or(Value::Null) {
        Value::Array(ids) => ids
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect(),
        _ => raw
            .get("projectId")
            .cloned()
            .unwrap_or(Value::Null)
            .as_str()
            .map_or_else(Vec::new, |p| vec![p.to_string()]),
    };
    SessionMeta {
        title: raw
            .get("title")
            .cloned()
            .unwrap_or(Value::Null)
            .as_str()
            .map(str::to_string),
        pinned: (raw.get("pinned").cloned().unwrap_or(Value::Null) == Value::Bool(true))
            .then_some(true),
        archived: (raw.get("archived").cloned().unwrap_or(Value::Null) == Value::Bool(true))
            .then_some(true),
        project_ids: Some(project_ids),
        model: match &raw.get("model").cloned().unwrap_or(Value::Null) {
            Value::Null => None,
            other => other.as_str().map(|s| Some(s.to_string())),
        },
        spans: Some(
            raw.get("spans")
                .cloned()
                .unwrap_or(Value::Null)
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter(|v| is_run_span(v))
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
        ),
        agent: raw
            .get("agent")
            .cloned()
            .unwrap_or(Value::Null)
            .as_str()
            .map(str::to_string),
        cwd: raw
            .get("cwd")
            .cloned()
            .unwrap_or(Value::Null)
            .as_str()
            .map(str::to_string),
        created_at: raw
            .get("createdAt")
            .cloned()
            .unwrap_or(Value::Null)
            .as_str()
            .map(str::to_string),
    }
}

struct Inner {
    path: PathBuf,
    data: MetaFile,
}

/// The overlay store — a single JSON file written atomically (tmp +
/// rename) after every mutation. A corrupt or half-written file degrades
/// to empty rather than failing.
#[derive(Clone)]
pub struct MetaStore {
    inner: Arc<Mutex<Inner>>,
}

impl MetaStore {
    /// # Panics
    /// On unwritable parent dirs at first flush.
    pub fn open(path: &Path) -> Self {
        let mut data = MetaFile::default();
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(&raw) {
                // v2: { sessions, projects, config } — else v1 flat
                // Record<sessionId, SessionMeta>.
                let raw_sessions = record
                    .get("sessions")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or(record.clone());
                let sessions = raw_sessions
                    .iter()
                    .map(|(id, value)| (id.clone(), normalize_session(value)))
                    .collect();
                let projects = record
                    .get("projects")
                    .and_then(Value::as_object)
                    .map(|p| {
                        p.iter()
                            .filter_map(|(id, v)| {
                                v.as_object()
                                    .and_then(|o| o["name"].as_str())
                                    .or_else(|| v.as_str())
                                    .map(|name| {
                                        (
                                            id.clone(),
                                            ProjectName {
                                                name: name.to_string(),
                                            },
                                        )
                                    })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let config = record
                    .get("config")
                    .and_then(Value::as_object)
                    .map(|c| c.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                data = MetaFile {
                    sessions,
                    projects,
                    config,
                };
            }
            // A corrupt or half-written file degrades to empty.
        }
        Self {
            inner: Arc::new(Mutex::new(Inner {
                path: path.to_path_buf(),
                data,
            })),
        }
    }

    fn flush(inner: &Inner) {
        if let Some(dir) = inner.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = inner
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        if let Ok(json) = serde_json::to_vec(&inner.data) {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &inner.path);
            }
        }
    }

    /// The overlay for one session (`None` = nothing recorded).
    pub fn of(&self, id: &str) -> Option<SessionMeta> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .data
            .sessions
            .get(id)
            .cloned()
    }

    /// Every recorded session meta, keyed by session id.
    pub fn sessions(&self) -> BTreeMap<String, SessionMeta> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .data
            .sessions
            .clone()
    }

    /// Merge a patch into one session's overlay.
    pub fn patch(&self, id: &str, patch: &SessionMeta) {
        let mut inner = lock_inner(&self.inner);
        inner
            .data
            .sessions
            .entry(id.to_string())
            .or_default()
            .apply(patch);
        Self::flush(&inner);
    }

    /// Record a run span; a no-op when it repeats the current agent+node.
    pub fn add_span(&self, id: &str, span: RunSpan) {
        let mut inner = lock_inner(&self.inner);
        let existing = inner.data.sessions.entry(id.to_string()).or_default();
        let next = append_span(existing.spans.as_deref().unwrap_or(&[]), span);
        if Some(&next) != existing.spans.as_ref() {
            existing.spans = Some(next);
            Self::flush(&inner);
        }
    }

    /// Drop a session's overlay entirely.
    pub fn remove(&self, id: &str) {
        let mut inner = lock_inner(&self.inner);
        if inner.data.sessions.remove(id).is_some() {
            Self::flush(&inner);
        }
    }

    pub fn list_projects(&self) -> Vec<Project> {
        lock_inner(&self.inner)
            .data
            .projects
            .iter()
            .map(|(id, p)| Project {
                id: id.clone(),
                name: p.name.clone(),
            })
            .collect()
    }

    pub fn create_project(&self, name: &str) -> Project {
        let id = format!("proj_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        self.ensure_project(&id, name)
    }

    /// Create the project under a caller-chosen id, or refresh its name
    /// when it already exists — the idempotent half of a project transfer.
    pub fn ensure_project(&self, id: &str, name: &str) -> Project {
        let mut inner = lock_inner(&self.inner);
        let existing = inner.data.projects.get(id);
        if existing.is_none_or(|p| p.name != name) {
            inner.data.projects.insert(
                id.to_string(),
                ProjectName {
                    name: name.to_string(),
                },
            );
            Self::flush(&inner);
        }
        Project {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    pub fn rename_project(&self, id: &str, name: &str) -> bool {
        let mut inner = lock_inner(&self.inner);
        let Some(entry) = inner.data.projects.get_mut(id) else {
            return false;
        };
        entry.name = name.to_string();
        Self::flush(&inner);
        true
    }

    pub fn delete_project(&self, id: &str) {
        let mut inner = lock_inner(&self.inner);
        if inner.data.projects.remove(id).is_none() {
            return;
        }
        for meta in inner.data.sessions.values_mut() {
            if let Some(ids) = &mut meta.project_ids {
                ids.retain(|p| p != id);
            }
        }
        Self::flush(&inner);
    }

    /// Server-persisted app/UI config.
    pub fn config(&self) -> BTreeMap<String, Value> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .data
            .config
            .clone()
    }

    pub fn set_config(&self, key: &str, value: Value) {
        let mut inner = lock_inner(&self.inner);
        inner.data.config.insert(key.to_string(), value);
        Self::flush(&inner);
    }
}
