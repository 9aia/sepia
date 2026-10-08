#![allow(clippy::unwrap_used, clippy::pedantic, clippy::missing_panics_doc)]

//! Control-plane tests against real spawned mock ACP agents — the
//! `AgentRuntime` fake only controls the spec's env, so lock probes,
//! capability gates, and the permission round-trip all go through real
//! subprocesses.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sepia_acp::{AcpConnection, PromptPart};
use sepia_control::{
    AgentRuntime, ControlErrorCode, ControlPlane, ControlPlaneOptions, HistoryOptions,
    RestoreRequest, RewindRequest, SessionRewinder, agent_for_backend,
};
use sepia_core::rewind::RewindPlan;
use sepia_core::{MessageNode, Role, Session, ToolCall};
use sepia_testkit::contract;
use serde_json::json;

mod common;
use common::*;

// ---------- list / history -------------------------------------------

#[tokio::test]
async fn maps_stored_sessions_to_summaries() {
    let (plane, _) = mk_plane(
        vec![
            contract::session("a", "Alpha", 100.0),
            contract::session("b", "Beta", 200.0),
        ],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let list = plane.list_sessions(false).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "b");
    assert!(!list[0].locked);
    assert!(!list[0].busy);
    plane.close_all().await;
}

#[tokio::test]
async fn merges_agent_lock_state_when_with_locks() {
    let (plane, _) = mk_plane(
        vec![contract::session("s1", "S1", 100.0)],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now|locked|4321")],
        )],
        ControlPlaneOptions::default(),
    );
    let list = plane.list_sessions(true).await.unwrap();
    assert!(list[0].locked);
    assert_eq!(list[0].lock_holder_pid, Some(4321.0));
    plane.close_all().await;
}

#[tokio::test]
async fn keeps_listing_when_the_lock_pass_fails() {
    // A spawn that fails contributes an empty view — the list still
    // returns stored sessions.
    struct DeadRuntime;
    #[async_trait]
    impl AgentRuntime for DeadRuntime {
        fn id(&self) -> &str {
            "devin"
        }
        fn label(&self) -> &str {
            "Dead"
        }
        async fn spawn(
            &self,
            _cwd: &str,
            _model: Option<&str>,
            _fallbacks: Option<&[String]>,
        ) -> Result<AcpConnection, sepia_acp::rpc::RpcError> {
            Err(sepia_acp::rpc::RpcError::internal("spawn failed"))
        }
    }
    let store = Arc::new(MemStore::new(vec![contract::session("s1", "S1", 100.0)]));
    let plane = ControlPlane::new(
        store,
        ControlPlaneOptions {
            agents: vec![Arc::new(DeadRuntime)],
            ..ControlPlaneOptions::default()
        },
    );
    let list = plane.list_sessions(true).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(!list[0].locked);
    plane.close_all().await;
}

#[tokio::test]
async fn cline_backend_maps_to_cline_agent() {
    assert_eq!(agent_for_backend("cline"), "cline");
    assert_eq!(agent_for_backend("claude"), "claude");
    assert_eq!(agent_for_backend("cursor"), "cursor");
    assert_eq!(agent_for_backend("windsurf"), "devin");
}

#[tokio::test]
async fn history_in_node_order_with_ms_timestamps() {
    let session = session_with(
        "devin",
        vec![node(1, Role::User, "one"), node(2, Role::Assistant, "two")],
    );
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let page = plane
        .get_history("s1", &HistoryOptions::default())
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.start, 0);
    assert_eq!(page.messages[0].content, "one");
    assert_eq!(page.messages[0].created_at, 1_700_000_001.0 * 1000.0);
    plane.close_all().await;
}

#[tokio::test]
async fn history_joins_tool_row_to_call_args_locations_diffs() {
    let mut assistant = node(1, Role::Assistant, "");
    assistant.tool_calls = vec![ToolCall {
        id: "tc1".into(),
        name: "write_file".into(),
        arguments: json!({"path": "/a.rs"}),
        index: 0,
        kind: "edit".into(),
        status: Some(sepia_core::ToolCallStatus::Success),
        exit_code: None,
        duration_ms: None,
        locations: vec![sepia_core::ToolCallLocation {
            path: "/a.rs".into(),
            line: None,
        }],
        diffs: vec![sepia_core::ToolCallDiff {
            path: "/a.rs".into(),
            old_text: None,
            new_text: Some("fn main() {}".into()),
        }],
    }];
    let mut tool = node(2, Role::Tool, "ok");
    tool.tool_call_id = Some("tc1".into());
    tool.tool_name = Some("write_file".into());
    let session = session_with("devin", vec![assistant, tool]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let page = plane
        .get_history("s1", &HistoryOptions::default())
        .await
        .unwrap();
    let tool_row = &page.messages[1];
    assert_eq!(tool_row.tool_call_id.as_deref(), Some("tc1"));
    assert_eq!(tool_row.args.as_deref(), Some("{\"path\":\"/a.rs\"}"));
    assert_eq!(tool_row.locations.as_ref().unwrap()[0].path, "/a.rs");
    assert_eq!(
        tool_row.diffs.as_ref().unwrap()[0].new_text.as_deref(),
        Some("fn main() {}")
    );
    plane.close_all().await;
}

#[tokio::test]
async fn history_paginates_from_the_tail() {
    let nodes: Vec<MessageNode> = (1..=10)
        .map(|i| node(i, Role::User, &format!("m{i}")))
        .collect();
    let session = session_with("devin", nodes);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let page = plane
        .get_history(
            "s1",
            &HistoryOptions {
                limit: Some(3),
                before: None,
                agent_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.total, 10);
    assert_eq!(page.start, 7);
    assert_eq!(page.messages[0].content, "m8");
    // `before` slices backward from that index.
    let page = plane
        .get_history(
            "s1",
            &HistoryOptions {
                limit: Some(2),
                before: Some(3),
                agent_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.messages[0].content, "m2");
    plane.close_all().await;
}

#[tokio::test]
async fn unknown_session_fails_not_found() {
    let (plane, _) = mk_plane(
        vec![],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .get_history("ghost", &HistoryOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::NotFound);
    let err = plane.get_session("ghost", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::NotFound);
    let err = plane
        .attach("ghost", false, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::NotFound);
    plane.close_all().await;
}

// ---------- attach ----------------------------------------------------

#[tokio::test]
async fn attach_spawns_agent_and_loads_session() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let runtime = MockRuntime::new("devin", &[("MOCK_SESSIONS", "s1|/work|S1|now")]);
    let spawns = Arc::clone(&runtime);
    let (plane, _) = mk_plane(vec![session], vec![runtime], ControlPlaneOptions::default());
    let result = plane.attach("s1", false, None, None, None).await.unwrap();
    assert!(result.attached);
    assert!(!result.read_only);
    assert_eq!(result.agent_id, "devin");
    assert!(result.capabilities.load_session);
    assert_eq!(spawns.spawns.load(Ordering::SeqCst), 1);
    // Re-attaching a live session does not spawn again.
    let again = plane.attach("s1", false, None, None, None).await.unwrap();
    assert!(again.attached);
    assert_eq!(spawns.spawns.load(Ordering::SeqCst), 1);
    plane.close_all().await;
}

#[tokio::test]
async fn locked_session_attaches_read_only_without_loading() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now|locked|777")],
        )],
        ControlPlaneOptions::default(),
    );
    let result = plane.attach("s1", false, None, None, None).await.unwrap();
    assert!(!result.attached);
    assert!(result.read_only);
    assert_eq!(result.agent_id, "devin");
    plane.close_all().await;
}

#[tokio::test]
async fn takeover_signals_the_reported_holder_then_loads() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let killed: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let killed_clone = Arc::clone(&killed);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now|locked|4242")],
        )],
        ControlPlaneOptions {
            terminate_lock_holder: Some(Arc::new(move |pid| {
                killed_clone.lock().unwrap().push(pid);
            })),
            ..ControlPlaneOptions::default()
        },
    );
    // The mock keeps reporting locked → the settle poll times out (~800ms),
    // then the load attempt arbitrates (and succeeds).
    let result = plane.attach("s1", true, None, None, None).await.unwrap();
    assert!(result.attached);
    assert_eq!(*killed.lock().unwrap(), vec![4242]);
    plane.close_all().await;
}

#[tokio::test]
async fn takeover_that_cannot_release_fails_locked() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_SESSIONS", "s1|/work|S1|now|locked|4242"),
                ("MOCK_LOAD_FAIL", "held"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .attach("s1", true, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Locked);
    assert!(err.message.contains("4242"));
    plane.close_all().await;
}

#[tokio::test]
async fn attach_prefers_the_sessions_own_agent() {
    let session = session_with("cline", vec![node(1, Role::User, "hi")]);
    let devin = MockRuntime::new("devin", &[]);
    let cline = MockRuntime::new("cline", &[("MOCK_SESSIONS", "s1|/work|S1|now")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![devin, cline],
        ControlPlaneOptions::default(),
    );
    let result = plane.attach("s1", false, None, None, None).await.unwrap();
    assert!(result.attached);
    assert_eq!(result.agent_id, "cline");
    plane.close_all().await;
}

#[tokio::test]
async fn load_failure_when_not_locked_propagates() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_SESSIONS", "s1|/work|S1|now"),
                ("MOCK_LOAD_FAIL", "corrupt"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .attach("s1", false, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Internal);
    plane.close_all().await;
}

#[tokio::test]
async fn agent_without_load_capability_fails_invalid() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_CAPS", "list,delete"),
                ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .attach("s1", false, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}

// ---------- prompt / cancel / permission ------------------------------

#[tokio::test]
async fn prompt_and_cancel_round_trip() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now")],
        )],
        ControlPlaneOptions::default(),
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let mut events = plane.subscribe("s1", None).await.unwrap();
    plane
        .prompt("s1", &[PromptPart::Text { text: "hi".into() }], None)
        .await
        .unwrap();
    // RunStarted + echo frames arrive on the event stream.
    let batch = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        batch
            .iter()
            .any(|e| matches!(e, sepia_proto::SessionEvent::RunStarted { .. }))
    );
    plane.cancel("s1", None).await.unwrap();
    plane.close_all().await;
}

#[tokio::test]
async fn prompt_rejects_content_the_agent_cannot_take() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_CAPS", "load,list"),
                ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let err = plane
        .prompt(
            "s1",
            &[PromptPart::Image {
                data: "aGk=".into(),
                mime_type: "image/png".into(),
                uri: None,
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    assert!(err.message.contains("image"));
    plane.close_all().await;
}

#[tokio::test]
async fn permission_response_resolves_and_unknown_ids_404() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now")],
        )],
        ControlPlaneOptions::default(),
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let mut events = plane.subscribe("s1", None).await.unwrap();
    plane
        .prompt(
            "s1",
            &[PromptPart::Text {
                text: "perm".into(),
            }],
            None,
        )
        .await
        .unwrap();
    // Fish the permission request out of the event stream.
    let request_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let batch = events.recv().await.unwrap();
            for event in batch {
                if let sepia_proto::SessionEvent::Custom { name, value } = event {
                    if name == "acp:permission_request" {
                        return value["requestId"].as_str().unwrap().to_string();
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    plane
        .respond_to_permission("s1", &request_id, Some("allow"), None)
        .await
        .unwrap();
    let err = plane
        .respond_to_permission("s1", "bogus", Some("allow"), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::NotFound);
    plane.close_all().await;
}

#[tokio::test]
async fn busy_session_refuses_a_second_prompt() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_SESSIONS", "s1|/work|S1|now"),
                ("MOCK_PROMPT_SLOW_MS", "300"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let plane2 = Arc::clone(&plane);
    let first = tokio::spawn(async move {
        plane2
            .prompt(
                "s1",
                &[PromptPart::Text {
                    text: "slow".into(),
                }],
                None,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let err = plane
        .prompt(
            "s1",
            &[PromptPart::Text {
                text: "second".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Busy);
    first.await.unwrap().unwrap();
    plane.close_all().await;
}

// ---------- delete / restore / rewind ---------------------------------

#[tokio::test]
async fn delete_goes_through_the_agent_runtime() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now")],
        )],
        ControlPlaneOptions::default(),
    );
    plane.delete_session("s1", None).await.unwrap();
    plane.close_all().await;
}

#[tokio::test]
async fn delete_fails_invalid_without_the_capability() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new(
            "devin",
            &[
                ("MOCK_CAPS", "load,list"),
                ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ],
        )],
        ControlPlaneOptions::default(),
    );
    let err = plane.delete_session("s1", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}

#[tokio::test]
async fn restore_requires_confirm_and_a_single_source() {
    let session = session_with("devin", vec![node(1, Role::User, "hi")]);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .restore(
            "s1",
            &RestoreRequest {
                confirm: false,
                path: Some("f.rs".into()),
                tool_call_id: None,
                checkpoint: None,
                paths: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    let err = plane
        .restore(
            "s1",
            &RestoreRequest {
                confirm: true,
                path: Some("f.rs".into()),
                tool_call_id: None,
                checkpoint: Some("abc".into()),
                paths: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}

#[tokio::test]
async fn restore_path_reverts_a_recorded_diff() {
    let dir = tempdir();
    let file = dir.path().join("f.rs");
    std::fs::write(&file, "after").unwrap();
    let mut session = session_with("devin", vec![]);
    session.working_directory = dir.path().to_string_lossy().to_string();
    let mut assistant = node(1, Role::Assistant, "");
    assistant.tool_calls = vec![ToolCall {
        id: "tc1".into(),
        name: "write".into(),
        arguments: json!({}),
        index: 0,
        kind: "edit".into(),
        status: Some(sepia_core::ToolCallStatus::Success),
        exit_code: None,
        duration_ms: None,
        locations: vec![],
        diffs: vec![sepia_core::ToolCallDiff {
            path: "f.rs".into(),
            old_text: Some("before".into()),
            new_text: Some("after".into()),
        }],
    }];
    session.nodes = vec![assistant];
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let result = plane
        .restore(
            "s1",
            &RestoreRequest {
                confirm: true,
                path: Some("f.rs".into()),
                tool_call_id: None,
                checkpoint: None,
                paths: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result.restored[0].action,
        sepia_control::RestoreAction::Written
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    plane.close_all().await;
}

#[tokio::test]
async fn restore_rejects_paths_outside_the_working_directory() {
    let dir = tempdir();
    let mut session = session_with("devin", vec![node(1, Role::User, "hi")]);
    session.working_directory = dir.path().to_string_lossy().to_string();
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .restore(
            "s1",
            &RestoreRequest {
                confirm: true,
                path: Some("../escape.rs".into()),
                tool_call_id: None,
                checkpoint: None,
                paths: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}

#[tokio::test]
async fn rewind_gates_and_delegates_to_the_backend_rewinder() {
    let nodes: Vec<MessageNode> = (1..=4)
        .flat_map(|i| {
            vec![
                node(i * 2 - 1, Role::User, &format!("u{i}")),
                node(i * 2, Role::Assistant, &format!("a{i}")),
            ]
        })
        .collect();
    let session = session_with("devin", nodes);
    // No rewinder registered → conflict.
    let (plane, _) = mk_plane(
        vec![session.clone()],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .rewind(
            "s1",
            &RewindRequest {
                confirm: true,
                node_id: None,
                turns: Some(1),
                checkpoint: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Conflict);
    // confirm gate.
    let err = plane
        .rewind(
            "s1",
            &RewindRequest {
                confirm: false,
                node_id: None,
                turns: Some(1),
                checkpoint: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;

    // With a rewinder — records the truncate call.
    struct RecordingRewinder {
        calls: Mutex<Vec<(usize, usize)>>,
    }
    #[async_trait]
    impl SessionRewinder for RecordingRewinder {
        async fn truncate(
            &self,
            _session: &Session,
            plan: &RewindPlan,
            _truncated: &Session,
        ) -> Result<(), sepia_control::ControlError> {
            self.calls
                .lock()
                .unwrap()
                .push((plan.keep_count, plan.removed.len()));
            Ok(())
        }
    }
    let rewinder = Arc::new(RecordingRewinder {
        calls: Mutex::new(Vec::new()),
    });
    let calls = Arc::clone(&rewinder);
    let (plane, _) = mk_plane(
        vec![session],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions {
            rewinders: HashMap::from([("devin".to_string(), rewinder as Arc<dyn SessionRewinder>)]),
            ..ControlPlaneOptions::default()
        },
    );
    let result = plane
        .rewind(
            "s1",
            &RewindRequest {
                confirm: true,
                node_id: None,
                turns: Some(1),
                checkpoint: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.removed, 2);
    assert_eq!(result.kept, 6);
    assert_eq!(calls.calls.lock().unwrap()[0], (6, 2));
    plane.close_all().await;
}

// ---------- create / agents -------------------------------------------

#[tokio::test]
async fn create_session_validates_cwd_and_agent() {
    let (plane, _) = mk_plane(
        vec![],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .create_session("relative/path", None, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    let err = plane
        .create_session("/tmp", Some("ghost".into()), None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::UnknownAgent);
    let created = plane
        .create_session("/tmp", None, None, None, None)
        .await
        .unwrap();
    assert_eq!(created.id, "mock-session");
    assert_eq!(created.agent_id, "devin");
    // Live but unflushed: history pages as empty, not not_found.
    let page = plane
        .get_history("mock-session", &HistoryOptions::default())
        .await
        .unwrap();
    assert_eq!(page.total, 0);
    plane.close_all().await;
}

#[tokio::test]
async fn list_agents_reports_probed_capabilities_after_spawn() {
    let (plane, _) = mk_plane(
        vec![session_with("devin", vec![node(1, Role::User, "hi")])],
        vec![MockRuntime::new(
            "devin",
            &[("MOCK_SESSIONS", "s1|/work|S1|now")],
        )],
        ControlPlaneOptions::default(),
    );
    let agents = plane.list_agents().await;
    assert!(agents[0].capabilities.is_none());
    plane.attach("s1", false, None, None, None).await.unwrap();
    let agents = plane.list_agents().await;
    assert!(agents[0].capabilities.as_ref().unwrap().load_session);
    plane.close_all().await;
}

#[tokio::test]
async fn subscribe_fails_when_not_attached() {
    let (plane, _) = mk_plane(
        vec![],
        vec![MockRuntime::new("devin", &[])],
        ControlPlaneOptions::default(),
    );
    let err = plane.subscribe("s1", None).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}
