//! SSR smoke test — the hub renders the session list server-side with
//! a stub `NodeApi`, no real node required.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use leptos::config::LeptosOptions;
use sepia_hub::{HubState, router};
use sepia_web::api::NodeApi;
use sepia_web::dto::{
    AgentCapabilitiesDto, AgentDto, AttachResultDto, CheckpointDto, CreateResultDto,
    HistoryPageDto, NodeInfoDto, NodeStatusDto, ProjectDto, PromptCapabilitiesDto,
    PushSubscriptionDto, SessionCapabilitiesDto, SessionSummaryDto,
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct StubNodeApi;

#[async_trait::async_trait]
impl NodeApi for StubNodeApi {
    async fn list_sessions(&self) -> Result<Vec<SessionSummaryDto>, String> {
        Ok(vec![
            SessionSummaryDto {
                id: "s1".into(),
                title: "Seeded Session Title".into(),
                cwd: "/tmp/seed".into(),
                agent: "devin".into(),
                updated_at: "2026-10-08T06:40:34.123Z".into(),
                ..SessionSummaryDto::default()
            },
            SessionSummaryDto {
                id: "s2".into(),
                title: "Second Seed".into(),
                ..SessionSummaryDto::default()
            },
        ])
    }

    async fn history(
        &self,
        _id: &str,
        _agent: Option<&str>,
        _before: Option<i64>,
        _limit: Option<usize>,
    ) -> Result<HistoryPageDto, String> {
        Ok(HistoryPageDto {
            start: 0,
            total: 0,
            ..HistoryPageDto::default()
        })
    }

    async fn get_session(
        &self,
        id: &str,
        _agent: Option<&str>,
    ) -> Result<SessionSummaryDto, String> {
        Ok(SessionSummaryDto {
            id: id.to_string(),
            title: "Seeded Session Title".into(),
            ..SessionSummaryDto::default()
        })
    }

    async fn prompt(&self, _id: &str, _agent: Option<&str>, _text: &str) -> Result<(), String> {
        Ok(())
    }

    async fn cancel(&self, _id: &str, _agent: Option<&str>) -> Result<(), String> {
        Ok(())
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
        Ok(vec![CheckpointDto {
            r#ref: "cp-1".into(),
            created_at: 1_791_441_634_123.0,
            run_count: Some(2),
            kind: Some("workspace".into()),
        }])
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
        Ok(vec![
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
                id: "cline".into(),
                label: "Cline".into(),
                capabilities: None,
                node: None,
            },
        ])
    }

    async fn list_projects(&self) -> Result<Vec<ProjectDto>, String> {
        Ok(vec![ProjectDto {
            id: "p1".into(),
            name: "Capstone Work".into(),
            node: None,
        }])
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
        Ok(BTreeMap::from([
            ("theme".to_string(), json!("dark")),
            ("historyLimit".to_string(), json!(50)),
        ]))
    }

    async fn set_config(&self, _key: &str, _value: &Value) -> Result<(), String> {
        Ok(())
    }

    async fn node_info(&self) -> Result<NodeInfoDto, String> {
        Ok(NodeInfoDto {
            id: "n1".into(),
            name: "workbench".into(),
            version: "0.1.0".into(),
            protocol: 1,
            agents: vec!["devin".into()],
            capabilities: vec!["sessions".into(), "push".into()],
        })
    }

    async fn node_status(&self) -> Result<Vec<NodeStatusDto>, String> {
        Ok(vec![
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
        ])
    }

    async fn push_vapid(&self) -> Result<String, String> {
        Ok("BJxNdlXStubVapidKeyForTestsOnly_0123456789abcdef".into())
    }

    async fn push_subscribe(&self, _subscription: &PushSubscriptionDto) -> Result<(), String> {
        Ok(())
    }

    async fn push_unsubscribe(&self, _endpoint: &str) -> Result<(), String> {
        Ok(())
    }
}

fn test_state() -> HubState {
    let options = LeptosOptions::builder()
        .output_name("sepia_web")
        .site_root(concat!(env!("CARGO_MANIFEST_DIR"), "/public"))
        .site_pkg_dir("pkg")
        .env(leptos::config::Env::DEV)
        .build();
    HubState::new(options, Arc::new(StubNodeApi), "http://127.0.0.1:9", None)
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, String) {
    let res = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn session_list_ssr_contains_seeded_title() {
    let app = router(test_state());
    let (status, html) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Seeded Session Title"),
        "SSR'd session list should contain the seeded title; got:\n{html}"
    );
}

#[tokio::test]
async fn session_detail_ssr_contains_seeded_title() {
    let app = router(test_state());
    let (status, html) = get(app, "/sessions/s1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Seeded Session Title"),
        "SSR'd session detail should contain the seeded title; got:\n{html}"
    );
}

#[tokio::test]
async fn style_route_serves_embedded_css() {
    let app = router(test_state());
    let (status, css) = get(app, "/style.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(css.contains("--"), "stylesheet should contain CSS vars");
}

#[tokio::test]
async fn manifest_is_served_from_site_root() {
    let app = router(test_state());
    let (status, body) = get(app, "/manifest.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"sepia\""));
}

#[tokio::test]
async fn agents_ssr_lists_stubbed_agents() {
    let app = router(test_state());
    let (status, html) = get(app, "/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Devin") && html.contains("Cline"),
        "SSR'd agents should contain the stubbed labels; got:\n{html}"
    );
    // Capability chips render from the wire's `capabilities` object.
    assert!(html.contains("loadSession"), "missing chip:\n{html}");
    assert!(html.contains("embeddedContext"), "missing chip:\n{html}");
}

#[tokio::test]
async fn projects_ssr_lists_stubbed_projects() {
    let app = router(test_state());
    let (status, html) = get(app, "/projects").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Capstone Work"),
        "SSR'd projects should contain the stubbed name; got:\n{html}"
    );
    assert!(html.contains("sessions"), "expected session count:\n{html}");
}

#[tokio::test]
async fn settings_ssr_renders_config_keys() {
    let app = router(test_state());
    let (status, html) = get(app, "/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("theme") && html.contains("historyLimit"),
        "SSR'd settings should contain the stubbed config keys; got:\n{html}"
    );
    assert!(
        html.contains("Enable notifications"),
        "push section should render the subscribe button; got:\n{html}"
    );
}

#[tokio::test]
async fn nodes_ssr_shows_identity_and_health() {
    let app = router(test_state());
    let (status, html) = get(app, "/nodes").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("workbench"),
        "SSR'd nodes should contain the node name; got:\n{html}"
    );
    assert!(
        html.contains("192.168.1.10"),
        "per-node health rows should render; got:\n{html}"
    );
    assert!(
        html.contains("down"),
        "node status should render; got:\n{html}"
    );
}

#[tokio::test]
async fn session_detail_ssr_renders_action_row() {
    let app = router(test_state());
    let (status, html) = get(app, "/sessions/s1").await;
    assert_eq!(status, StatusCode::OK);
    // Unlocked + not live → a plain Attach (the takeover label only
    // shows for locked sessions); rename/delete/checkpoints always ride.
    // (SSR wraps dynamic text in `<!--hk-->` markers — match substrings.)
    assert!(html.contains("Attach"), "missing Attach button:\n{html}");
    assert!(
        !html.contains("Attach (takeover)"),
        "unlocked session should not offer takeover:\n{html}"
    );
    assert!(html.contains("Rename"), "missing Rename button:\n{html}");
    assert!(
        html.contains("Checkpoints"),
        "missing Checkpoints button:\n{html}"
    );
    assert!(html.contains("Delete"), "missing Delete button:\n{html}");
}

#[tokio::test]
async fn session_list_ssr_renders_create_form() {
    let app = router(test_state());
    let (status, html) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("new-session"),
        "SSR'd list should contain the create form; got:\n{html}"
    );
    assert!(
        html.contains("Working directory"),
        "create form should carry the cwd input; got:\n{html}"
    );
    assert!(
        html.contains("default agent"),
        "agent select should render its fallback option; got:\n{html}"
    );
}
