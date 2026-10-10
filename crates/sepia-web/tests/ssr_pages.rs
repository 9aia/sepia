//! Page-level SSR tests — `App` rendered to HTML in-process against
//! `FakeNodeApi`, no hub/axum/node/Chrome. Server fns run inline
//! (the `ssr` build) and resolve `Arc<dyn NodeApi>` from leptos
//! context, exactly as `sepia-hub` provides it per request.
//!
//! The harness mirrors `leptos_axum::build_response`: a fresh root
//! `Owner` with an `SsrSharedContext`, the request path handed to the
//! router via `RequestUrl`, then `to_html_stream_in_order` collected
//! so every `Suspense`/`Suspend` resolves before we assert.

#![cfg(feature = "ssr")]
#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::sync::Arc;

use futures::StreamExt;
use hydration_context::SsrSharedContext;
use leptos::prelude::*;
use leptos_router::location::RequestUrl;
use sepia_web::App;
use sepia_web::api::NodeApi;
use sepia_web::testing::FakeNodeApi;

/// SSR-render `App` at `path` against `api` and return the HTML body
/// fragment (no outer shell — the shell lives in `sepia-hub`).
async fn render(path: &str, api: Arc<dyn NodeApi>) -> String {
    let owner = Owner::new_root(Some(Arc::new(SsrSharedContext::new())));
    let stream = owner.with(|| {
        provide_context(RequestUrl::new(path));
        provide_context::<Arc<dyn NodeApi>>(api);
        view! { <App/> }.to_html_stream_in_order()
    });
    let html = stream.collect::<String>().await;
    // Don't leak the owner across tests — drop it once the stream
    // has drained (mirrors `build_response`'s end-of-stream cleanup).
    drop(owner);
    html
}

/// `spawn_local` (used by suspense resources and leptos-fetch) goes
/// through `any_spawner`; on tokio that needs a `LocalSet` in scope.
async fn page(path: &str, api: Arc<dyn NodeApi>) -> String {
    let _ = any_spawner::Executor::init_tokio();
    tokio::task::LocalSet::new()
        .run_until(render(path, api))
        .await
}

fn assert_contains(html: &str, needle: &str) {
    assert!(
        html.contains(needle),
        "expected {needle:?} in rendered page; got:\n{html}"
    );
}

// ── `/` — master-detail session list ─────────────────────────────────

#[tokio::test]
async fn session_list_renders_titles_badges_and_groups() {
    let html = page("/", FakeNodeApi::seeded().shared()).await;
    for title in ["Fix flaky login spec", "Locked refactor plan", "Docs sweep"] {
        assert_contains(&html, title);
    }
    // Count line + cwd group headings (last path segment per group).
    assert_contains(&html, "3 sessions");
    assert_contains(&html, "acme-api");
    assert_contains(&html, "docs-site");
    // Status chips from the seeded flags.
    assert_contains(&html, ">busy<");
    assert_contains(&html, ">locked<");
    // The empty chat pane shows its placeholder on lg+.
    assert_contains(&html, "Select a session");
}

#[tokio::test]
async fn session_list_renders_create_form() {
    let html = page("/", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "<form");
    assert_contains(&html, "Working directory");
    assert_contains(&html, "default agent");
}

#[tokio::test]
async fn session_list_error_state_renders_error_banner() {
    let html = page(
        "/",
        FakeNodeApi::seeded()
            .with_error("fake backend exploded")
            .shared(),
    )
    .await;
    assert_contains(&html, "data-name=\"ErrorBanner\"");
    assert_contains(&html, "role=\"alert\"");
    assert_contains(&html, "fake backend exploded");
}

#[tokio::test]
async fn session_list_empty_renders_hint() {
    let html = page("/", FakeNodeApi::seeded().with_sessions(vec![]).shared()).await;
    assert_contains(&html, "No sessions yet.");
}

// ── `/sessions/:id` — detail page (panel + transcript) ───────────────

#[tokio::test]
async fn session_detail_renders_summary_history_and_prompt() {
    let html = page("/sessions/s1", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Fix flaky login spec");
    // Badges: live + busy + the agent chip.
    assert_contains(&html, ">live<");
    assert_contains(&html, ">busy<");
    assert_contains(&html, ">devin<");
    assert_contains(&html, "/work/acme-api");
    // Action row: a live session offers Detach + Cancel run.
    assert_contains(&html, "Detach");
    assert_contains(&html, "Cancel run");
    // History: user text, assistant text, reasoning block, tool row.
    assert_contains(&html, "please fix the flaky login spec");
    assert_contains(&html, "On it — the fixture DB needs per-test isolation.");
    assert_contains(&html, ">thinking<");
    assert_contains(&html, "run_command");
    assert_contains(&html, "cargo test -p acme-api");
    // The prompt composer.
    assert_contains(&html, "<textarea");
    assert_contains(&html, "Message the agent");
    assert_contains(&html, ">Send<");
}

#[tokio::test]
async fn session_detail_locked_offers_takeover() {
    let html = page("/sessions/s2", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Locked refactor plan");
    assert_contains(&html, ">locked<");
    assert_contains(&html, "Attach (takeover)");
}

#[tokio::test]
async fn session_detail_error_state_renders_error_banner() {
    let html = page(
        "/sessions/s1",
        FakeNodeApi::seeded()
            .with_error("node unreachable")
            .shared(),
    )
    .await;
    assert_contains(&html, "data-name=\"ErrorBanner\"");
    assert_contains(&html, "node unreachable");
}

#[tokio::test]
async fn list_with_selected_session_embeds_panel() {
    // `?session=` mounts SessionPanel in the master-detail pane.
    let html = page("/?session=s3&agent=devin", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Docs sweep");
    assert_contains(&html, "Message the agent");
}

// ── `/agents` ─────────────────────────────────────────────────────────

#[tokio::test]
async fn agents_renders_inventory_and_capability_chips() {
    let html = page("/agents", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Agents");
    assert_contains(&html, "Devin");
    assert_contains(&html, "Claude");
    assert_contains(&html, "loadSession");
    assert_contains(&html, "embeddedContext");
    // `claude` has no probed capabilities.
    assert_contains(&html, "capabilities not probed");
}

// ── `/projects` ───────────────────────────────────────────────────────

#[tokio::test]
async fn projects_renders_names_and_session_counts() {
    let html = page("/projects", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Projects");
    assert_contains(&html, "Capstone Work");
    assert_contains(&html, "Infra");
    // `p1` is referenced by s1's `projectIds`.
    assert_contains(&html, "1 sessions");
    assert_contains(&html, "New project name");
}

// ── `/nodes` ──────────────────────────────────────────────────────────

#[tokio::test]
async fn nodes_renders_identity_health_and_empty_outbox() {
    let html = page("/nodes", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "workbench");
    assert_contains(&html, "0.1.0 (protocol 1)");
    // Health rows from `node_status`.
    assert_contains(&html, ">down<");
    assert_contains(&html, "192.168.1.10");
    // Seeded outbox is empty.
    assert_contains(&html, "Queued writes");
    assert_contains(&html, "No queued writes.");
}

#[tokio::test]
async fn nodes_renders_pending_writes() {
    use sepia_web::dto::PendingWriteDto;
    let api = FakeNodeApi::seeded()
        .with_pending(vec![PendingWriteDto {
            id: "w1".into(),
            node_id: "tower".into(),
            session_id: "s1".into(),
            op: "prompt".into(),
            kind: "turn".into(),
            status: "failed".into(),
            enqueued_at: "2026-10-08T06:45:00.000Z".into(),
            attempts: 5,
            last_error: Some("node down".into()),
        }])
        .shared();
    let html = page("/nodes", api).await;
    assert_contains(&html, ">failed<");
    assert_contains(&html, "prompt");
    assert_contains(&html, "tower → s1");
}

// ── `/settings` ───────────────────────────────────────────────────────

#[tokio::test]
async fn settings_renders_all_sections() {
    let html = page("/settings", FakeNodeApi::seeded().shared()).await;
    for section in [
        "Settings",
        "Appearance",
        "Node",
        "Agents",
        "Queued writes",
        "Configuration",
        "Push notifications",
        "Keyboard shortcuts",
    ] {
        assert_contains(&html, section);
    }
    // Node section + config rows come from the fake.
    assert_contains(&html, "workbench");
    assert_contains(&html, "theme");
    assert_contains(&html, "historyLimit");
    assert_contains(&html, "Enable notifications");
}

// ── router ────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_renders_fallback() {
    let html = page("/nope", FakeNodeApi::seeded().shared()).await;
    assert_contains(&html, "Page not found");
}
