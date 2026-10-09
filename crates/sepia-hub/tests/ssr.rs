//! SSR smoke test — the hub renders the session list server-side with
//! a stub `NodeApi`, no real node required.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use leptos::config::LeptosOptions;
use sepia_hub::{HubConfig, HubState, router};
use sepia_web::api::NodeApi;
use sepia_web::dto::{
    AgentCapabilitiesDto, AgentDto, AttachResultDto, CheckpointDto, CreateResultDto,
    HistoryPageDto, NodeInfoDto, NodeStatusDto, PendingWriteDto, ProjectDto, PromptCapabilitiesDto,
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

    async fn fs_dirs(&self, _path: &str, _node: Option<String>) -> Result<Vec<String>, String> {
        Ok(vec!["/tmp/seed".into(), "/tmp/seed/sub".into()])
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

    async fn pending_writes(&self) -> Result<Vec<PendingWriteDto>, String> {
        Ok(vec![
            PendingWriteDto {
                id: "w1".into(),
                node_id: "tower".into(),
                session_id: "s1".into(),
                op: "prompt".into(),
                kind: "turn".into(),
                status: "queued".into(),
                enqueued_at: "2026-10-08T06:45:00.000Z".into(),
                attempts: 1,
                last_error: None,
            },
            PendingWriteDto {
                id: "w2".into(),
                node_id: "tower".into(),
                session_id: "s2".into(),
                op: "meta.patch".into(),
                kind: "metadata".into(),
                status: "failed".into(),
                enqueued_at: "2026-10-08T06:46:00.000Z".into(),
                attempts: 5,
                last_error: Some("expired".into()),
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

/// The hub's own token — unrelated to any node credential.
const HUB_TOKEN: &str = "test-hub-token";

fn authed_state() -> HubState {
    let mut state = test_state();
    state.hub_token = Some(Arc::from(HUB_TOKEN));
    state
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, String) {
    let ((status, _), body) = request(app, uri, &[]).await;
    (status, body)
}

/// Full response — status, headers, body — for auth assertions.
async fn request(
    app: axum::Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> ((StatusCode, HeaderMap), String) {
    let mut builder = Request::builder().uri(uri);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let res = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        (status, headers),
        String::from_utf8_lossy(&bytes).into_owned(),
    )
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
        html.contains("<form"),
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

// ── browser → hub auth (SEPIA_HUB_TOKEN) ─────────────────────────────

#[tokio::test]
async fn unauthenticated_request_is_unauthorized() {
    let app = router(authed_state());
    let ((status, headers), body) = request(app, "/", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["www-authenticate"], "Bearer");
    assert!(
        body.contains("\"unauthorized\""),
        "401 body should be the json error; got:\n{body}"
    );
    // Non-page routes gate identically.
    let app = router(authed_state());
    let (status, _) = get(app, "/sessions/s1").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let app = router(authed_state());
    let ((status, _), _) = request(app, "/api/events", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn query_token_authenticates_and_sets_cookie() {
    let app = router(authed_state());
    let ((status, headers), html) = request(app, "/?token=test-hub-token", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Seeded Session Title"));
    let cookie = headers["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with("sepia_hub=test-hub-token"),
        "query token should plant the cookie; got: {cookie}"
    );
    assert!(cookie.contains("HttpOnly"), "cookie must be HttpOnly");
}

#[tokio::test]
async fn wrong_token_is_unauthorized() {
    for headers in [
        vec![("authorization", "Bearer wrong")],
        vec![("cookie", "sepia_hub=wrong")],
        vec![],
    ] {
        let app = router(authed_state());
        let ((status, _), _) = request(app, "/?token=wrong", &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "headers: {headers:?}");
    }
}

#[tokio::test]
async fn bearer_header_and_cookie_authenticate() {
    let app = router(authed_state());
    let (status, _) = get_with(app, "/", &[("authorization", "Bearer test-hub-token")]).await;
    assert_eq!(status, StatusCode::OK);

    let app = router(authed_state());
    let (status, _) = get_with(app, "/", &[("cookie", "sepia_hub=test-hub-token; other=x")]).await;
    assert_eq!(status, StatusCode::OK);
}

async fn get_with(app: axum::Router, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let ((status, _), body) = request(app, uri, headers).await;
    (status, body)
}

#[tokio::test]
async fn static_assets_stay_open() {
    // The PWA shell + service worker must load before any credential.
    for path in ["/style.css", "/manifest.json", "/sw.js", "/icon.svg"] {
        let app = router(authed_state());
        let (status, _) = get(app, path).await;
        assert_eq!(status, StatusCode::OK, "{path} should stay open");
    }
    // The wasm bundle dir is likewise ungated (404 here — no pkg staged
    // in the test site root — but never 401).
    let app = router(authed_state());
    let (status, _) = get(app, "/pkg/sepia_web.js").await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_validates_then_redirects_with_cookie() {
    let app = router(authed_state());
    let ((status, headers), _) = request(app, "/login?token=test-hub-token", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/");
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("sepia_hub=")
    );

    let app = router(authed_state());
    let (status, _) = get(app, "/login?token=wrong").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn sse_bridge_accepts_query_token() {
    // EventSource can't set headers. The stub upstream is unreachable
    // (127.0.0.1:9), so a passed gate surfaces as 502, never 401.
    let app = router(authed_state());
    let (status, _) = get(app, "/api/events?token=test-hub-token").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let app = router(authed_state());
    let (status, _) = get(app, "/api/sessions/s1/stream?token=test-hub-token").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn hub_fns_require_auth_too() {
    // Leptos server fns ride the browser cookie same-origin — no creds,
    // no call.
    let app = router(authed_state());
    let ((status, _), _) = request(app, "/hub/list_sessions", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let app = router(authed_state());
    let ((status, _), _) = request(
        app,
        "/hub/list_sessions",
        &[("cookie", "sepia_hub=test-hub-token")],
    )
    .await;
    // Past the gate — the body/route itself may still 404/405, but auth
    // must not be what fails it.
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tokenless_hub_stays_open() {
    // Loopback-only mode — every surface behaves exactly as before.
    let app = router(test_state());
    let (status, html) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Seeded Session Title"));
}

#[test]
fn hub_config_refuses_non_loopback_without_token() {
    use std::collections::HashMap;
    let cfg = |pairs: &[(&str, &str)]| {
        HubConfig::from_map(
            &pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<HashMap<_, _>>(),
        )
    };
    for host in ["0.0.0.0", "::", "192.168.1.10"] {
        let err = cfg(&[("SEPIA_HUB_HOST", host)]).unwrap_err();
        assert!(
            err.contains("SEPIA_HUB_TOKEN"),
            "{host} should demand the token: {err}"
        );
        assert!(cfg(&[("SEPIA_HUB_HOST", host), ("SEPIA_HUB_TOKEN", "t")]).is_ok());
    }
    for host in ["127.0.0.1", "localhost", "::1", "127.0.0.2"] {
        assert!(
            cfg(&[("SEPIA_HUB_HOST", host)]).is_ok(),
            "{host} may serve tokenless"
        );
    }
    // Default bind stays open; the token parses through.
    let c = cfg(&[("SEPIA_HUB_TOKEN", "secret")]).unwrap();
    assert_eq!(c.hub_token.as_deref(), Some("secret"));
    assert_eq!(c.host, "127.0.0.1");
    // Empty token counts as unset.
    assert!(cfg(&[("SEPIA_HUB_HOST", "0.0.0.0"), ("SEPIA_HUB_TOKEN", "")]).is_err());
}

// ── outbox visibility (queued/failed writes) ─────────────────────────

#[tokio::test]
async fn session_list_ssr_marks_queued_writes() {
    let app = router(test_state());
    let (status, html) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    // The stub queues a `prompt` for s1 and a dead-lettered
    // `meta.patch` for s2 — both badges render on their rows.
    assert!(
        html.contains("queued"),
        "SSR'd list should carry the queued badge; got:\n{html}"
    );
    assert!(
        html.contains("failed"),
        "SSR'd list should carry the failed badge; got:\n{html}"
    );
}

#[tokio::test]
async fn session_detail_ssr_shows_queued_badge() {
    let app = router(test_state());
    let (status, html) = get(app, "/sessions/s1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("queued"),
        "SSR'd detail should carry the queued badge; got:\n{html}"
    );
}

#[tokio::test]
async fn nodes_ssr_lists_queued_writes() {
    let app = router(test_state());
    let (status, html) = get(app, "/nodes").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Queued writes"),
        "nodes page should have the queued-writes section; got:\n{html}"
    );
    // Pending op row + the dead-letter's error text.
    assert!(html.contains("prompt"), "queued op should list: {html}");
    assert!(
        html.contains("expired"),
        "dead-letter error should render; got:\n{html}"
    );
}
