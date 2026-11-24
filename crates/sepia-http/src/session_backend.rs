//! `SessionPlaneBackend` — peregrin's [`sepia_peregrin::SessionBackend`]
//! for `kind="sepia-session"`, implemented over the node's own
//! [`SessionPlane`](sepia_session::SessionPlane) plus the [`SessionState`] side-channels the session
//! routes own (meta overlay, event feed, held-watch, live listeners,
//! convert/import seams).
//!
//! This replaces the old `SessionEntityPlane`, which re-invoked the
//! `/api/sessions*` handlers through an `SessionState` host downcast. The
//! sepia-side plane owns the entity contract; this backend keeps the
//! *peregrin* semantics those handlers carried — the `PEREGRIN_META`
//! overlay on list/get, pending (created-but-unflushed) rows, the
//! feed/`held`/`live` bookkeeping on attach/create/delete/rewind/
//! convert/import, and the store-delete fallback for runtime-less
//! agents — so `/api/entities*` behaves exactly as it did when the
//! ops round-tripped through the session routes.
//!
//! Works uniformly over [`sepia_session::LocalPlane`] (in-process)
//! and the `sepia serve` wire plane peregrin-session provides — both
//! already expose every verb the trait needs.

use sepia_acp::{AvailableCommand, PromptPart};
use sepia_core::Session;
use sepia_meta::{RunSpan, SessionMeta};
use sepia_peregrin::SessionBackend;
use sepia_session::{
    AttachResult, ControlError, ControlErrorCode, CreateResult, ImportOverrides, PromptWaitResult,
    RestoreRequest, RestoreResult, RewindRequest, RewindResult, SessionSummary,
};
use serde_json::{Value, json};

use crate::SessionState;
use crate::routes::now_ms_f64;
use crate::routes::sessions::{ImportTarget, list_for_entities, summary_for_entities};

/// Decode a session-route wire row (`SummaryWire` JSON) back into the
/// typed [`SessionSummary`] the entity doc's `indexDoc` carries. The
/// wire shape is a superset — `model` drops out (the summary type has
/// no such field), everything else round-trips.
fn summary_of(wire: Value) -> Result<SessionSummary, ControlError> {
    serde_json::from_value(wire).map_err(|e| {
        ControlError::new(
            ControlErrorCode::Internal,
            format!("session summary decode failed: {e}"),
        )
    })
}

/// The `sepia-session` entity backend — see the module docs. Holds a
/// clone of the node [`SessionState`] (all `Arc` fields — cheap) so entity
/// ops share the session routes' overlay/feed/watch state.
pub struct SessionPlaneBackend {
    state: SessionState,
}

impl SessionPlaneBackend {
    /// A backend over `state`'s session plane and side-channels.
    #[must_use]
    pub fn new(state: &SessionState) -> Self {
        Self {
            state: state.clone(),
        }
    }

    /// The `DELETE /api/sessions/{id}` tail — held-watch release, the
    /// `deleted` feed event, and the meta-overlay removal — shared by
    /// `delete_session` and the store-level fallback.
    fn after_delete(&self, id: &str, agent: Option<&str>) {
        self.state.held.unwatch(id, agent);
        self.state
            .feed
            .emit_session(id, agent, json!({ "deleted": true }));
        if let Some(meta) = &self.state.meta {
            meta.remove(id);
        }
    }

    /// The `created` feed event + live-listener registration +
    /// created-but-unflushed meta record the `POST /api/sessions`
    /// route writes after a successful create — delegated to the
    /// route's shared `track_created` so both surfaces stay identical.
    fn after_create(
        &self,
        id: &str,
        agent_id: &str,
        cwd: &str,
        title: Option<String>,
        model: Option<String>,
    ) {
        crate::routes::sessions::track_created(&self.state, id, agent_id, cwd, title, model);
    }
}

#[async_trait::async_trait]
impl SessionBackend for SessionPlaneBackend {
    async fn list_sessions(&self, with_locks: bool) -> Result<Vec<SessionSummary>, ControlError> {
        // `list_for_entities` is the `/api/sessions` merge — store rows
        // + the `PEREGRIN_META` overlay + pending meta-only rows.
        let rows = list_for_entities(&self.state, with_locks).await?;
        rows.into_iter().map(summary_of).collect()
    }

    async fn summary(&self, id: &str, agent: Option<&str>) -> Result<SessionSummary, ControlError> {
        summary_of(summary_for_entities(&self.state, id, agent).await?)
    }

    async fn session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError> {
        self.state.plane.get_session(id, agent).await
    }

    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError> {
        let created = self
            .state
            .plane
            .create_session(cwd, agent, title.clone(), model.clone(), fallbacks)
            .await?;
        self.after_create(&created.id, &created.agent_id, cwd, title, model);
        Ok(created)
    }

    async fn attach(
        &self,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent: Option<String>,
    ) -> Result<AttachResult, ControlError> {
        match self
            .state
            .plane
            .attach(id, takeover, model, fallbacks, agent.clone())
            .await
        {
            Err(e) => {
                // A failed takeover leaves the session held — keep the
                // watch so the release edge still reaches the feed.
                if e.code == ControlErrorCode::Locked {
                    self.state.held.watch(id, agent.as_deref());
                }
                Err(e)
            }
            Ok(result) => {
                self.state.live.register(
                    &self.state.plane,
                    &self.state.feed,
                    self.state.push.clone(),
                    id,
                    agent.clone(),
                );
                self.state.feed.emit_session(
                    id,
                    Some(&result.agent_id),
                    json!({ "live": result.attached }),
                );
                // Read-only means another process holds the store lock.
                if result.read_only {
                    self.state.held.watch(id, Some(&result.agent_id));
                } else if result.attached {
                    self.state.held.unwatch(id, Some(&result.agent_id));
                }
                // Run provenance — the same span the attach route writes.
                if result.attached {
                    if let Some(meta) = &self.state.meta {
                        meta.add_span(
                            id,
                            RunSpan {
                                at: now_ms_f64(),
                                agent: result.agent_id.clone(),
                                node: self.state.node_id(),
                            },
                        );
                    }
                }
                Ok(result)
            }
        }
    }

    async fn detach(&self, id: &str) {
        self.state.plane.detach(id).await;
    }

    async fn prompt(
        &self,
        id: &str,
        parts: &[PromptPart],
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.state.plane.prompt(id, parts.to_vec(), agent).await
    }

    async fn prompt_wait(
        &self,
        id: &str,
        parts: &[PromptPart],
        agent: Option<&str>,
        timeout: Option<std::time::Duration>,
    ) -> Result<PromptWaitResult, ControlError> {
        self.state
            .plane
            .prompt_wait(id, parts.to_vec(), agent, timeout)
            .await
    }

    async fn commands(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<Vec<AvailableCommand>, ControlError> {
        self.state.plane.commands(id, agent).await
    }

    async fn command(
        &self,
        id: &str,
        name: &str,
        args: Option<&str>,
        text: Option<&str>,
        timeout: Option<std::time::Duration>,
        agent: Option<&str>,
    ) -> Result<PromptWaitResult, ControlError> {
        self.state
            .plane
            .command(id, name, args, text, timeout, agent)
            .await
    }

    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        self.state.plane.cancel(id, agent).await
    }

    async fn permission(
        &self,
        id: &str,
        request_id: &str,
        option_id: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.state
            .plane
            .respond_to_permission(id, request_id, option_id, agent)
            .await
    }

    async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent: Option<&str>,
    ) -> Result<RestoreResult, ControlError> {
        self.state.plane.restore(id, request, agent).await
    }

    async fn rewind(
        &self,
        id: &str,
        request: &RewindRequest,
        agent: Option<&str>,
    ) -> Result<RewindResult, ControlError> {
        let result = self.state.plane.rewind(id, request, agent).await?;
        self.state
            .feed
            .emit_session(id, agent, json!({ "rewound": true }));
        Ok(result)
    }

    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        // `DELETE /api/sessions/{id}` semantics: a created-but-unflushed
        // session isn't in the repo — deleting it is still a success.
        let result = match self.state.plane.delete_session(id, agent).await {
            Ok(()) => Ok(()),
            Err(e) if e.code == ControlErrorCode::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        result?;
        self.after_delete(id, agent);
        Ok(())
    }

    async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError> {
        self.state.plane.delete_session_store(agent, id).await?;
        self.after_delete(id, Some(agent));
        Ok(())
    }

    async fn patch_meta(&self, id: &str, patch: &SessionMeta) -> Result<(), ControlError> {
        let Some(meta) = &self.state.meta else {
            return Err(ControlError::new(
                ControlErrorCode::Internal,
                "meta overlay is not configured on this server",
            ));
        };
        meta.patch(id, patch);
        Ok(())
    }

    async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError> {
        // The `/api/sessions/import` provenance writes — `created` feed
        // event + run span — on whatever id the import landed as.
        let new_id = self
            .state
            .plane
            .import_session(agent, session, overrides)
            .await?;
        self.state
            .feed
            .emit_session(&new_id, Some(agent), json!({ "created": true }));
        if let Some(meta) = &self.state.meta {
            meta.add_span(
                &new_id,
                RunSpan {
                    at: now_ms_f64(),
                    agent: agent.to_string(),
                    node: self.state.node_id(),
                },
            );
        }
        Ok(new_id)
    }

    async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError> {
        // The wired convert seam carries the cline↔devin `from`
        // inference — prefer it for the built-in targets when the
        // caller didn't pin a source store; anything else goes
        // straight to the plane.
        if from.is_none() {
            let target = match agent {
                "cline" => Some(ImportTarget::Cline),
                "devin" => Some(ImportTarget::Devin),
                _ => None,
            };
            if let (Some(target), Some(convert)) = (target, &self.state.convert) {
                let new_id = convert(id.to_string(), target).await?;
                self.state
                    .feed
                    .emit_session(&new_id, Some(agent), json!({ "created": true }));
                return Ok(new_id);
            }
        }
        let new_id = self.state.plane.convert_session(id, agent, from).await?;
        self.state
            .feed
            .emit_session(&new_id, Some(agent), json!({ "created": true }));
        Ok(new_id)
    }
}
