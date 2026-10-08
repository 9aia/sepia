//! Store-union helpers: `agent_for_backend` plus `MergedRepository`, the
//! primary-store + read-only-overlays composition.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use sepia_core::storage::{NodesWindowOptions, SessionNodeWindow, SessionRepository};
use sepia_core::{Session, StorageError};

/// Maps a store backend to the agent that can resume it. `"cursor"` maps
/// to itself even though no ACP runtime exists — keeping the id distinct
/// lets the control plane refuse attach rather than graft the session
/// onto the default agent.
pub fn agent_for_backend(backend_type: &str) -> &str {
    match backend_type {
        "cline" | "claude" | "cursor" => backend_type,
        _ => "devin",
    }
}

/// Overlay one primary repository with extra read sources. Extra-repo
/// failures degrade to "empty / not found" so a broken overlay can't
/// take down the primary listing; primary errors propagate.
pub struct MergedRepository {
    primary: Arc<dyn SessionRepository>,
    extras: Vec<Arc<dyn SessionRepository>>,
}

impl MergedRepository {
    /// The stores themselves narrow `agent_id` lookups against their own
    /// backend — merged's job is precedence (primary first) + degradation
    /// (an extra's failure reads as "not found").
    pub fn new(
        primary: Arc<dyn SessionRepository>,
        extras: Vec<Arc<dyn SessionRepository>>,
    ) -> Self {
        Self { primary, extras }
    }
}

#[async_trait]
impl SessionRepository for MergedRepository {
    async fn save(&self, session: &Session) -> Result<(), StorageError> {
        self.primary.save(session).await
    }

    async fn get_by_id(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        if let Some(found) = self.primary.get_by_id(id, agent_id).await? {
            return Ok(Some(found));
        }
        for repo in &self.extras {
            if let Ok(Some(hit)) = repo.get_by_id(id, agent_id).await {
                return Ok(Some(hit));
            }
        }
        Ok(None)
    }

    async fn list(&self) -> Result<Vec<Session>, StorageError> {
        let mut all = self.primary.list().await?;
        for repo in &self.extras {
            if let Ok(extra) = repo.list().await {
                all.extend(extra);
            }
        }
        // Primary wins on id collisions.
        let mut seen = HashSet::new();
        all.retain(|s| seen.insert(s.id.clone()));
        all.sort_by(|a, b| b.last_activity_at.total_cmp(&a.last_activity_at));
        Ok(all)
    }

    async fn summary(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Option<Session>, StorageError> {
        if let Some(found) = self.primary.summary(id, agent_id).await? {
            return Ok(Some(found));
        }
        for repo in &self.extras {
            if let Ok(Some(hit)) = repo.summary(id, agent_id).await {
                return Ok(Some(hit));
            }
        }
        Ok(None)
    }

    async fn nodes_window(
        &self,
        id: &str,
        options: &NodesWindowOptions,
    ) -> Result<Option<SessionNodeWindow>, StorageError> {
        if let Some(hit) = self.primary.nodes_window(id, options).await? {
            return Ok(Some(hit));
        }
        for repo in &self.extras {
            if let Ok(Some(hit)) = repo.nodes_window(id, options).await {
                return Ok(Some(hit));
            }
        }
        Ok(None)
    }

    async fn delete(&self, id: &str) -> Result<(), StorageError> {
        self.primary.delete(id).await
    }

    async fn has_session(&self, id: &str) -> Result<bool, StorageError> {
        if self.primary.has_session(id).await? {
            return Ok(true);
        }
        for repo in &self.extras {
            if repo.has_session(id).await.unwrap_or(false) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
