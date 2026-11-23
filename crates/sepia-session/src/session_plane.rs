//! `SessionPlane` — the session control port the HTTP surface and
//! the `sepia-session` entity backend consume. Implementable
//! in-process over [`crate::ControlPlane`] or over the `sepia serve`
//! wire — the trait names only sepia types, so any host (peregrin's
//! node, a test harness) can mount the session REST routes without
//! linking session internals.

use std::sync::Arc;
use std::time::Duration;

use sepia_acp::{AvailableCommand, PromptPart};
use sepia_core::Session;
use sepia_proto::SessionEvent;
use tokio::sync::broadcast;

use crate::{
    AgentInfo, AttachResult, ControlError, ControlErrorCode, ControlPlane, CreateResult,
    HistoryOptions, HistoryPage, ImportOverrides, Local, PromptWaitResult, RestoreRequest,
    RestoreResult, RewindRequest, RewindResult, SessionSummary,
};

/// Everything the session surface can ask of the session plane — the
/// same surface `ControlPlane` exposes, implementable in-process or
/// over `sepia serve`.
#[async_trait::async_trait]
pub trait SessionPlane: Send + Sync {
    /// Sessions across every registered store; `with_locks` probes
    /// agents for live lock state.
    async fn list_sessions(&self, with_locks: bool) -> Result<Vec<SessionSummary>, ControlError>;
    /// Full session payload.
    async fn get_session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError>;
    /// Metadata-only session (no nodes/prompt history).
    async fn get_summary(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError>;
    /// Paginated history.
    async fn get_history(
        &self,
        id: &str,
        options: &HistoryOptions,
        agent: Option<&str>,
    ) -> Result<HistoryPage, ControlError>;
    /// Attach to a session — `takeover` preempts a live holder.
    async fn attach(
        &self,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent: Option<String>,
    ) -> Result<AttachResult, ControlError>;
    /// Create a session under `cwd` on an agent.
    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError>;
    /// Detach a held session.
    async fn detach(&self, id: &str);
    /// Forward a prompt to the holder.
    async fn prompt(
        &self,
        id: &str,
        parts: Vec<PromptPart>,
        agent: Option<&str>,
    ) -> Result<(), ControlError>;
    /// Prompt and block until the turn settles — the one-call
    /// orchestrator primitive (`POST /api/sessions/run`). `timeout`
    /// bounds the wait; `None` waits until the turn ends.
    async fn prompt_wait(
        &self,
        id: &str,
        parts: Vec<PromptPart>,
        agent: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<PromptWaitResult, ControlError>;
    /// Switch a live session's agent mode (`sessions.set_mode` → ACP
    /// `session/set_mode`) — the unattended-run knob for agents that
    /// gate tool use behind permission prompts.
    async fn set_mode(
        &self,
        id: &str,
        mode_id: &str,
        agent: Option<&str>,
    ) -> Result<(), ControlError>;
    /// Cancel a running turn.
    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError>;
    /// Resolve a pending permission request.
    async fn respond_to_permission(
        &self,
        id: &str,
        request_id: &str,
        option: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), ControlError>;
    /// Delete a session from its store.
    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError>;
    /// Store-level delete bypassing the agent runtime — the fallback
    /// for agents with no `session/delete` ACP capability. `sepia
    /// serve`'s `sessions.delete` already performs this fallback
    /// internally, so the service impl re-issues the same call.
    async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError>;
    /// `sessions.import` — write session IR into `agent`'s store;
    /// `overrides` re-key/patch the copy. Returns the new session id.
    async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError>;
    /// `sessions.convert` — node-local store→store copy of `id` into
    /// `agent`'s store (`from` disambiguates the source store).
    /// Returns the new session id.
    async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError>;
    /// Restore rewindable files.
    async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent: Option<&str>,
    ) -> Result<RestoreResult, ControlError>;
    /// Rewind messages/history.
    async fn rewind(
        &self,
        id: &str,
        request: &RewindRequest,
        agent: Option<&str>,
    ) -> Result<RewindResult, ControlError>;
    /// Live event stream for an attached session.
    async fn subscribe(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError>;
    /// Non-pinning event stream — same batches as [`Self::subscribe`]
    /// but the receiver never counts as a listener for sepia's idle
    /// sweep, so background consumers (busy-tracking forwarders, log
    /// mirrors) can't hold the agent subprocess open past the TTL.
    /// Defaults to [`Self::subscribe`] for planes without a watch
    /// channel.
    async fn watch(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError> {
        self.subscribe(id, agent).await
    }
    /// Registered agents + last probed capabilities.
    async fn list_agents(&self) -> Vec<AgentInfo>;
    /// The live session's advertised slash commands — the agent's
    /// last `available_commands_update`; empty when it never sent one.
    async fn commands(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<Vec<AvailableCommand>, ControlError>;
    /// Invoke one advertised slash command as a prompt turn —
    /// `name` must be advertised (`unsupported` otherwise); `args`
    /// appends to the `/{name}` line, `text` overrides it verbatim.
    async fn command(
        &self,
        id: &str,
        name: &str,
        args: Option<&str>,
        text: Option<&str>,
        timeout: Option<Duration>,
        agent: Option<&str>,
    ) -> Result<PromptWaitResult, ControlError>;
    /// Whether the session's owning agent supports rewind/restore.
    async fn can_rewind(&self, backend_type: &str) -> bool;
    /// Graceful shutdown — detach everything.
    async fn close_all(&self);
}

fn control(code: ControlErrorCode, e: impl std::fmt::Display) -> ControlError {
    ControlError::new(code, e.to_string())
}

/// In-process plane — `Arc<ControlPlane>` passthrough plus, when built
/// from a `Local`, its driver registry: import/convert need a
/// write-capable driver store, so a bare-`ControlPlane` construction
/// (unit-test planes) errors `internal` on those two. Used by tests
/// and `PEREGRIN_SESSION_PLANE=local`/`inprocess`.
pub struct LocalPlane(pub Arc<ControlPlane>, pub Option<Local>);

#[async_trait::async_trait]
impl SessionPlane for LocalPlane {
    async fn list_sessions(&self, with_locks: bool) -> Result<Vec<SessionSummary>, ControlError> {
        self.0.list_sessions(with_locks).await
    }
    async fn get_session(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError> {
        self.0.get_session(id, agent).await
    }
    async fn get_summary(&self, id: &str, agent: Option<&str>) -> Result<Session, ControlError> {
        self.0.get_summary(id, agent).await
    }
    async fn get_history(
        &self,
        id: &str,
        options: &HistoryOptions,
        agent: Option<&str>,
    ) -> Result<HistoryPage, ControlError> {
        let _ = agent;
        self.0.get_history(id, options).await
    }
    async fn attach(
        &self,
        id: &str,
        takeover: bool,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
        agent: Option<String>,
    ) -> Result<AttachResult, ControlError> {
        self.0.attach(id, takeover, model, fallbacks, agent).await
    }
    async fn create_session(
        &self,
        cwd: &str,
        agent: Option<String>,
        title: Option<String>,
        model: Option<String>,
        fallbacks: Option<Vec<String>>,
    ) -> Result<CreateResult, ControlError> {
        self.0
            .create_session(cwd, agent, title, model, fallbacks)
            .await
    }
    async fn detach(&self, id: &str) {
        self.0.detach(id).await;
    }
    async fn prompt(
        &self,
        id: &str,
        parts: Vec<PromptPart>,
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.0.prompt(id, &parts, agent).await
    }
    async fn prompt_wait(
        &self,
        id: &str,
        parts: Vec<PromptPart>,
        agent: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<PromptWaitResult, ControlError> {
        self.0.prompt_wait(id, &parts, agent, timeout).await
    }
    async fn set_mode(
        &self,
        id: &str,
        mode_id: &str,
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.0.set_mode(id, mode_id, agent).await
    }
    async fn cancel(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        self.0.cancel(id, agent).await
    }
    async fn respond_to_permission(
        &self,
        id: &str,
        request_id: &str,
        option: Option<&str>,
        agent: Option<&str>,
    ) -> Result<(), ControlError> {
        self.0
            .respond_to_permission(id, request_id, option, agent)
            .await
    }
    async fn delete_session(&self, id: &str, agent: Option<&str>) -> Result<(), ControlError> {
        self.0.delete_session(id, agent).await
    }
    async fn delete_session_store(&self, agent: &str, id: &str) -> Result<(), ControlError> {
        let Some(local) = &self.1 else {
            return Err(control(
                ControlErrorCode::Internal,
                "store delete needs a write-capable driver store — this plane was built without a Local",
            ));
        };
        local.delete_session_store(agent, id).await
    }
    async fn import_session(
        &self,
        agent: &str,
        session: &Session,
        overrides: &ImportOverrides,
    ) -> Result<String, ControlError> {
        let Some(local) = &self.1 else {
            return Err(control(
                ControlErrorCode::Internal,
                "import needs a write-capable driver store — this plane was built without a Local",
            ));
        };
        local.import_session(agent, session, overrides).await
    }
    async fn convert_session(
        &self,
        id: &str,
        agent: &str,
        from: Option<&str>,
    ) -> Result<String, ControlError> {
        let Some(local) = &self.1 else {
            return Err(control(
                ControlErrorCode::Internal,
                "convert needs a write-capable driver store — this plane was built without a Local",
            ));
        };
        local.convert_session(id, agent, from).await
    }
    async fn restore(
        &self,
        id: &str,
        request: &RestoreRequest,
        agent: Option<&str>,
    ) -> Result<RestoreResult, ControlError> {
        self.0.restore(id, request, agent).await
    }
    async fn rewind(
        &self,
        id: &str,
        request: &RewindRequest,
        agent: Option<&str>,
    ) -> Result<RewindResult, ControlError> {
        self.0.rewind(id, request, agent).await
    }
    async fn subscribe(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError> {
        self.0.subscribe(id, agent).await
    }
    async fn watch(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<broadcast::Receiver<Vec<SessionEvent>>, ControlError> {
        self.0.watch(id, agent).await
    }
    async fn list_agents(&self) -> Vec<AgentInfo> {
        self.0.list_agents().await
    }
    async fn commands(
        &self,
        id: &str,
        agent: Option<&str>,
    ) -> Result<Vec<AvailableCommand>, ControlError> {
        self.0.session_commands(id, agent).await
    }
    async fn command(
        &self,
        id: &str,
        name: &str,
        args: Option<&str>,
        text: Option<&str>,
        timeout: Option<Duration>,
        agent: Option<&str>,
    ) -> Result<PromptWaitResult, ControlError> {
        self.0
            .session_command(id, name, args, text, timeout, agent)
            .await
    }
    async fn can_rewind(&self, backend_type: &str) -> bool {
        self.0.can_rewind(backend_type)
    }
    async fn close_all(&self) {
        self.0.close_all().await;
    }
}
