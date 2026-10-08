//! SSR smoke test — the hub renders the session list server-side with
//! a stub `NodeApi`, no real node required.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use leptos::config::LeptosOptions;
use sepia_hub::{HubState, router};
use sepia_web::api::NodeApi;
use sepia_web::dto::{HistoryPageDto, SessionSummaryDto};
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
