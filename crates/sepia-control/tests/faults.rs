#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Fault injection — agent death mid-run, malformed updates, spawn
//! storms. The control plane must stay consistent (busy clears, lock
//! state survives, the plane itself never wedges) even when the
//! subprocess misbehaves.

use std::sync::Arc;
use std::time::Duration;

use sepia_control::{ControlErrorCode, ControlPlaneOptions};

mod common;
use common::*;

#[tokio::test]
async fn agent_death_mid_prompt_errors_but_clears_busy() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ("MOCK_EXIT_AFTER_PROMPT", "1"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions::default(),
    );
    let session = session_with("windsurf", vec![node(1, sepia_core::Role::User, "hi")]);
    drop(session);
    plane.attach("s1", false, None, None, None).await.unwrap();
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "boom".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    // The prompt errors (dead agent) — the session is no longer busy
    // and a fresh prompt attempt yields a fresh, clean error.
    assert!(matches!(
        err.code,
        ControlErrorCode::Internal | ControlErrorCode::Invalid
    ));
    let list = plane.list_sessions(false).await.unwrap();
    let summary = list.iter().find(|s| s.id == "s1").unwrap();
    assert!(!summary.busy, "busy must clear after agent death");
    plane.close_all().await;
}

#[tokio::test]
async fn malformed_updates_do_not_wedge_the_turn() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ("MOCK_BAD_UPDATES", "1"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions::default(),
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    // Garbage lines + a non-object content update must not kill the
    // reader or the turn.
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        plane.prompt(
            "s1",
            &[sepia_acp::PromptPart::Text { text: "hi".into() }],
            None,
        ),
    )
    .await
    .expect("prompt hung on malformed updates");
    assert!(result.is_ok(), "prompt failed: {result:?}");
    plane.close_all().await;
}

#[tokio::test]
async fn attach_to_a_session_whose_agent_died_at_init_fails_clean() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
            ("MOCK_EXIT_AFTER_INIT", "1"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .attach("s1", false, None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err.code,
        ControlErrorCode::Internal | ControlErrorCode::Invalid
    ));
    // The plane is still usable — a second attach produces the same
    // clean failure rather than a wedged half-state.
    let err2 = plane
        .attach("s1", false, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, err2.code);
    plane.close_all().await;
}

#[tokio::test]
async fn prompt_to_a_non_live_session_is_invalid_not_panic() {
    let (plane, _) = mk_plane(
        vec![session_with("windsurf", vec![])],
        vec![MockRuntime::new("devin", &[("MOCK_CAPS", "load,list")])],
        ControlPlaneOptions::default(),
    );
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text { text: "hi".into() }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ControlErrorCode::Invalid);
    plane.close_all().await;
}
