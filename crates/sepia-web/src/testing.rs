//! Test fixtures — a deterministic in-memory [`NodeApi`] for SSR
//! tests. `FakeNodeApi::seeded()` returns a fixed world (three
//! sessions across two cwd groups, two agents, two projects, a
//! history page, checkpoints, node info, an empty outbox); the
//! `.with_*` knobs override individual fixtures and `.with_error(..)`
//! makes every read endpoint fail so pages render their error states.
//!
//! Only compiled under `ssr` — `NodeApi` itself is an ssr-gated port.
//! Integration tests render pages with this in leptos context; the
//! hub's own tests keep their local `StubNodeApi` (different shape —
//! it proves the axum wiring, this one feeds real markup).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::api::NodeApi;
use sepia_core::{TokenUsage, ToolCallDiff, ToolCallLocation};

use crate::dto::{
    AgentCapabilitiesDto, AgentDto, AttachResultDto, CheckpointDto, CreateResultDto,
    HistoryMessageDto, HistoryPageDto, NodeInfoDto, NodeStatusDto, PendingWriteDto, ProjectDto,
    PromptCapabilitiesDto, PushSubscriptionDto, RunSpanDto, SessionCapabilitiesDto,
    SessionSummaryDto,
};

/// A `NodeApi` that answers every read from in-memory fixtures.
/// Mutations are no-ops returning plausible successes — the SSR tests
/// only exercise reads, but the trait wants the whole surface.
pub struct FakeNodeApi {
    sessions: Vec<SessionSummaryDto>,
    agents: Vec<AgentDto>,
    projects: Vec<ProjectDto>,
    history: HistoryPageDto,
    checkpoints: Vec<CheckpointDto>,
    config: BTreeMap<String, Value>,
    info: NodeInfoDto,
    statuses: Vec<NodeStatusDto>,
    pending: Vec<PendingWriteDto>,
    /// When `Some`, every read endpoint returns it as `Err`.
    error: Option<String>,
}

impl FakeNodeApi {
    /// The canonical fixture world — what `GET /api/*` on a healthy
    /// dev node looks like.
    ///
    /// Sessions: `s1` is live+busy on `devin` with a sub-agent child
    /// (`s6`), `s2` is store-locked, `s3` is plain, `s4` is pinned,
    /// `s5` is archived (hidden until the list's toggle). Two
    /// distinct cwds produce two group headings.
    pub fn seeded() -> Self {
        Self {
            sessions: vec![
                SessionSummaryDto {
                    id: "s1".into(),
                    title: "Fix flaky login spec".into(),
                    cwd: "/work/acme-api".into(),
                    agent: "devin".into(),
                    updated_at: "2026-10-08T06:40:34.123Z".into(),
                    busy: true,
                    live: true,
                    project_ids: vec!["p1".into()],
                    // Two provenance spans: the first heads the
                    // transcript, the second marks a mid-transcript
                    // transfer → both render marker rows.
                    spans: vec![
                        RunSpanDto {
                            at: 1_791_441_500_000.0,
                            agent: "devin".into(),
                            node: "workbench".into(),
                        },
                        RunSpanDto {
                            at: 1_791_441_615_000.0,
                            agent: "devin".into(),
                            node: "tower".into(),
                        },
                    ],
                    ..SessionSummaryDto::default()
                },
                SessionSummaryDto {
                    id: "s2".into(),
                    title: "Locked refactor plan".into(),
                    cwd: "/work/acme-api".into(),
                    agent: "claude".into(),
                    updated_at: "2026-10-08T05:10:00.000Z".into(),
                    locked: true,
                    lock_holder_pid: Some(4242.0),
                    ..SessionSummaryDto::default()
                },
                SessionSummaryDto {
                    id: "s3".into(),
                    title: "Docs sweep".into(),
                    cwd: "/work/docs-site".into(),
                    agent: "devin".into(),
                    updated_at: "2026-10-07T18:02:11.000Z".into(),
                    ..SessionSummaryDto::default()
                },
                SessionSummaryDto {
                    id: "s4".into(),
                    title: "Keep me on top".into(),
                    cwd: "/work/docs-site".into(),
                    agent: "claude".into(),
                    updated_at: "2026-10-08T07:00:00.000Z".into(),
                    pinned: true,
                    ..SessionSummaryDto::default()
                },
                SessionSummaryDto {
                    id: "s5".into(),
                    title: "Stale spike".into(),
                    cwd: "/work/acme-api".into(),
                    agent: "devin".into(),
                    updated_at: "2026-10-06T12:00:00.000Z".into(),
                    archived: true,
                    ..SessionSummaryDto::default()
                },
                SessionSummaryDto {
                    id: "s6".into(),
                    title: "Scan test matrix".into(),
                    cwd: "/work/acme-api".into(),
                    agent: "devin".into(),
                    updated_at: "2026-10-08T06:45:00.000Z".into(),
                    parent_session_id: Some("s1".into()),
                    ..SessionSummaryDto::default()
                },
            ],
            agents: vec![
                AgentDto {
                    id: "devin".into(),
                    label: "Devin".into(),
                    capabilities: Some(AgentCapabilitiesDto {
                        load_session: true,
                        session_list: true,
                        prompt_capabilities: PromptCapabilitiesDto {
                            image: true,
                            audio: false,
                            embedded_context: true,
                        },
                        session_capabilities: SessionCapabilitiesDto {
                            delete: true,
                            ..SessionCapabilitiesDto::default()
                        },
                    }),
                    node: None,
                },
                AgentDto {
                    id: "claude".into(),
                    label: "Claude".into(),
                    capabilities: None,
                    node: None,
                },
            ],
            projects: vec![
                ProjectDto {
                    id: "p1".into(),
                    name: "Capstone Work".into(),
                    node: None,
                },
                ProjectDto {
                    id: "p2".into(),
                    name: "Infra".into(),
                    node: None,
                },
            ],
            history: HistoryPageDto {
                start: 0,
                total: 8,
                messages: vec![
                    // System blobs fold into the ContextCard — one
                    // `<system_info>` and one `<rules>` row.
                    HistoryMessageDto {
                        role: "system".into(),
                        node_id: 0,
                        content: "<system_info>\nThe following information is automatically \
                                  generated context about your current environment.\nCurrent \
                                  workspace directories:\n  /work/acme-api (cwd)\n\nPlatform: \
                                  linux\n</system_info>"
                            .into(),
                        created_at: 1_791_441_590_000.0,
                        ..HistoryMessageDto::default()
                    },
                    HistoryMessageDto {
                        role: "system".into(),
                        node_id: 1,
                        content: "<rules type=\"always-on\">\n<rule name=\"global_rules\" \
                                  path=\"/x/global_rules.md\">\nbe nice\n</rule>\n</rules>"
                            .into(),
                        created_at: 1_791_441_591_000.0,
                        ..HistoryMessageDto::default()
                    },
                    HistoryMessageDto {
                        role: "user".into(),
                        node_id: 2,
                        content: "please fix the flaky login spec".into(),
                        created_at: 1_791_441_600_000.0,
                        ..HistoryMessageDto::default()
                    },
                    HistoryMessageDto {
                        role: "assistant".into(),
                        node_id: 3,
                        content: "On it — the fixture DB needs per-test isolation.".into(),
                        thinking: Some(
                            "The spec flakes because two tests share one database.".into(),
                        ),
                        usage: Some(TokenUsage {
                            input: 4200.0,
                            output: 88.0,
                            cache_read: Some(1024.0),
                            cost: Some(0.013),
                            ..TokenUsage::default()
                        }),
                        created_at: 1_791_441_610_000.0,
                        ..HistoryMessageDto::default()
                    },
                    HistoryMessageDto {
                        role: "tool".into(),
                        node_id: 4,
                        content: "42 passed, 0 failed".into(),
                        tool_name: Some("run_command".into()),
                        tool_status: Some("success".into()),
                        exit_code: Some(0),
                        args: Some(json!({"command": "cargo test -p acme-api"}).to_string()),
                        created_at: 1_791_441_620_000.0,
                        ..HistoryMessageDto::default()
                    },
                    // An edit row carrying recorded diffs + locations —
                    // exercises ToolEdit + DiffBlock.
                    HistoryMessageDto {
                        role: "tool".into(),
                        node_id: 5,
                        content: "The file /work/acme-api/tests/login.rs has been updated.\n\n\
                                  edited file:\nlet db = TestDb::isolated();"
                            .into(),
                        tool_name: Some("edit_file".into()),
                        tool_status: Some("success".into()),
                        args: Some(
                            json!({"file_path": "/work/acme-api/tests/login.rs"}).to_string(),
                        ),
                        diffs: Some(vec![ToolCallDiff {
                            path: "/work/acme-api/tests/login.rs".into(),
                            old_text: Some("let db = shared_db();".into()),
                            new_text: Some("let db = TestDb::isolated();".into()),
                        }]),
                        locations: Some(vec![ToolCallLocation {
                            path: "/work/acme-api/tests/login.rs".into(),
                            line: Some(14),
                        }]),
                        created_at: 1_791_441_625_000.0,
                        ..HistoryMessageDto::default()
                    },
                    // Back-to-back identical assistant rows fold — the
                    // surviving row carries a `×2` marker and a GFM
                    // table.
                    HistoryMessageDto {
                        role: "assistant".into(),
                        node_id: 6,
                        content: "Fixed — each test now gets its own database.\n\n\
                                  | File | Status |\n| --- | --- |\n| db.rs | fixed |"
                            .into(),
                        created_at: 1_791_441_630_000.0,
                        ..HistoryMessageDto::default()
                    },
                    HistoryMessageDto {
                        role: "assistant".into(),
                        node_id: 7,
                        content: "Fixed — each test now gets its own database.\n\n\
                                  | File | Status |\n| --- | --- |\n| db.rs | fixed |"
                            .into(),
                        created_at: 1_791_441_631_000.0,
                        ..HistoryMessageDto::default()
                    },
                ],
            },
            checkpoints: vec![
                CheckpointDto {
                    r#ref: "cp-run1".into(),
                    created_at: 1_791_441_600_000.0,
                    run_count: Some(1),
                    kind: Some("workspace".into()),
                },
                CheckpointDto {
                    r#ref: "cp-run2".into(),
                    created_at: 1_791_441_700_000.0,
                    run_count: Some(2),
                    kind: Some("transcript".into()),
                },
            ],
            config: BTreeMap::from([
                ("theme".to_string(), json!("dark")),
                ("historyLimit".to_string(), json!(50)),
            ]),
            info: NodeInfoDto {
                id: "n1".into(),
                name: "workbench".into(),
                version: "0.1.0".into(),
                protocol: 1,
                agents: vec!["devin".into(), "claude".into()],
                capabilities: vec!["sessions".into(), "push".into()],
            },
            statuses: vec![
                NodeStatusDto {
                    id: "n1".into(),
                    url: "http://127.0.0.1:8787".into(),
                    label: "workbench".into(),
                    status: "up".into(),
                    last_seen_at: Some("2026-10-08T06:40:34.123Z".into()),
                },
                NodeStatusDto {
                    id: "tower".into(),
                    url: "http://192.168.1.10:8787".into(),
                    label: "tower".into(),
                    status: "down".into(),
                    last_seen_at: None,
                },
            ],
            pending: Vec::new(),
            error: None,
        }
    }

    /// Every read endpoint returns `Err(msg)` — pages render their
    /// `ErrorBanner` fallback instead of data.
    #[must_use]
    pub fn with_error(mut self, msg: impl Into<String>) -> Self {
        self.error = Some(msg.into());
        self
    }

    /// Replace the outbox rows (e.g. to render the queued/failed
    /// badges on `/nodes` and the session list).
    #[must_use]
    pub fn with_pending(mut self, pending: Vec<PendingWriteDto>) -> Self {
        self.pending = pending;
        self
    }

    /// Replace the session list (e.g. `Vec::new()` for the empty
    /// "No sessions yet." state).
    #[must_use]
    pub fn with_sessions(mut self, sessions: Vec<SessionSummaryDto>) -> Self {
        self.sessions = sessions;
        self
    }

    /// Replace the node health rows (e.g. `Vec::new()` or all-`down`
    /// for the "No nodes connected" gate on the session list).
    #[must_use]
    pub fn with_statuses(mut self, statuses: Vec<NodeStatusDto>) -> Self {
        self.statuses = statuses;
        self
    }

    /// Convenience — `Arc<dyn NodeApi>` is what server fns pull from
    /// leptos context.
    pub fn shared(self) -> std::sync::Arc<dyn NodeApi> {
        std::sync::Arc::new(self)
    }

    /// The shared failure check for the read endpoints below.
    fn fail(&self) -> Result<(), String> {
        match &self.error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl NodeApi for FakeNodeApi {
    async fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>, String> {
        self.fail()?;
        Ok(self.sessions.clone())
    }

    async fn history(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _before: Option<i64>,
        _limit: Option<usize>,
    ) -> Result<HistoryPageDto, String> {
        self.fail()?;
        Ok(self.history.clone())
    }

    async fn get_session(
        &self,
        id: &str,
        _agent: Option<&str>,
    ) -> Result<SessionSummaryDto, String> {
        self.fail()?;
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| format!("session {id} not found"))
    }

    async fn create_session(
        &self,
        _cwd: &str,
        agent: Option<&str>,
        _title: Option<&str>,
        _model: Option<&str>,
        _node: Option<&str>,
    ) -> Result<CreateResultDto, String> {
        Ok(CreateResultDto {
            id: "s-new".into(),
            agent_id: agent.unwrap_or("devin").to_string(),
        })
    }

    async fn attach(
        &self,
        _id: &str,
        agent: Option<&str>,
        _takeover: bool,
    ) -> Result<AttachResultDto, String> {
        Ok(AttachResultDto {
            attached: true,
            read_only: false,
            agent_id: agent.unwrap_or("devin").to_string(),
        })
    }

    async fn detach(&self, _id: &str, _agent: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    async fn prompt(&self, _id: &str, _agent: Option<&str>, _text: &str) -> Result<(), String> {
        Ok(())
    }

    async fn cancel(&self, _id: &str, _agent: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    async fn answer_permission(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _request_id: &str,
        _option_id: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn patch_meta(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _patch: &Value,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn delete_session(&self, _id: &str, _agent: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    async fn checkpoints(
        &self,
        _id: &str,
        _agent: Option<&str>,
    ) -> Result<Vec<CheckpointDto>, String> {
        self.fail()?;
        Ok(self.checkpoints.clone())
    }

    async fn restore(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _checkpoint: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn rewind(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _checkpoint: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn list_agents(&self) -> Result<Vec<AgentDto>, String> {
        self.fail()?;
        Ok(self.agents.clone())
    }

    async fn list_projects(&self) -> Result<Vec<ProjectDto>, String> {
        self.fail()?;
        Ok(self.projects.clone())
    }

    async fn create_project(&self, name: &str, _node: Option<&str>) -> Result<ProjectDto, String> {
        Ok(ProjectDto {
            id: "p-new".into(),
            name: name.to_string(),
            node: None,
        })
    }

    async fn delete_project(&self, _id: &str, _node: Option<&str>) -> Result<(), String> {
        Ok(())
    }

    async fn get_config(&self) -> Result<BTreeMap<String, Value>, String> {
        self.fail()?;
        Ok(self.config.clone())
    }

    async fn set_config(&self, _key: &str, _value: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn fs_dirs(&self, _path: &str, _node: Option<String>) -> Result<Vec<String>, String> {
        self.fail()?;
        Ok(vec!["/work/acme-api".into(), "/work/docs-site".into()])
    }

    async fn pair(&self, _code: &str, _node: Option<&str>) -> Result<String, String> {
        self.fail()?;
        Ok("sepia-pair-fake-token-0123456789".into())
    }

    async fn node_info(&self) -> Result<NodeInfoDto, String> {
        self.fail()?;
        Ok(self.info.clone())
    }

    async fn rename_node(&self, name: &str, _node: Option<&str>) -> Result<NodeInfoDto, String> {
        let mut info = self.info.clone();
        info.name = name.to_string();
        Ok(info)
    }

    async fn node_status(&self) -> Result<Vec<NodeStatusDto>, String> {
        self.fail()?;
        Ok(self.statuses.clone())
    }

    async fn pending_writes(&self) -> Result<Vec<PendingWriteDto>, String> {
        self.fail()?;
        Ok(self.pending.clone())
    }

    async fn push_vapid(&self) -> Result<String, String> {
        self.fail()?;
        Ok("BJxNdlXFakeVapidKeyForTestsOnly_0123456789abcdef".into())
    }

    async fn push_subscribe(&self, _subscription: &PushSubscriptionDto) -> Result<(), String> {
        Ok(())
    }

    async fn push_unsubscribe(&self, _endpoint: &str) -> Result<(), String> {
        Ok(())
    }
}
