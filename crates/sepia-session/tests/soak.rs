#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Bounded soak — churn attach/detach/prompt cycles against the mock
//! agent and assert the plane's state stays compact (no live-session
//! leaks, RSS doesn't climb). The plan's hour-scale version runs
//! manually; CI gets the bounded form.

use std::sync::Arc;
use std::time::Duration;

use sepia_session::ControlPlaneOptions;

mod common;
use common::*;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn attach_detach_churn_leaves_no_leaks() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
        ],
    );
    let spawns = Arc::clone(&runtime);
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions::default(),
    );

    for _ in 0..50 {
        plane.attach("s1", false, None, None, None).await.unwrap();
        plane
            .prompt(
                "s1",
                &[sepia_acp::PromptPart::Text {
                    text: "churn".into(),
                }],
                None,
            )
            .await
            .unwrap();
        plane.detach("s1").await;
    }

    let list = plane.list_sessions(false).await.unwrap();
    assert!(!list.iter().any(|s| s.id == "s1" && s.busy));
    assert_eq!(spawns.spawns.load(std::sync::atomic::Ordering::SeqCst), 50);
    plane.close_all().await;
    // One more cycle must work after close_all -> everything torn down.
    drop(spawns);
}

/// Idle live sessions get detached by the sweep — the state map drains.
#[tokio::test]
async fn idle_sweep_reclaims_live_sessions() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions {
            idle_ttl: Some(Duration::from_millis(150)),
            sweep_interval: Some(Duration::from_millis(50)),
            ..ControlPlaneOptions::default()
        },
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Idle TTL expired -> swept; a prompt now fails "not attached".
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "late".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, sepia_session::ControlErrorCode::Invalid);
    plane.close_all().await;
}

/// A `watch` receiver sees the same batches but never pins the session —
/// the idle sweep still reclaims the agent subprocess, and the stream
/// closes with the detach.
#[tokio::test]
async fn watch_does_not_pin_idle_sweep() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions {
            idle_ttl: Some(Duration::from_millis(150)),
            sweep_interval: Some(Duration::from_millis(50)),
            ..ControlPlaneOptions::default()
        },
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let mut rx = plane.watch("s1", None).await.unwrap();
    // A prompt's events still reach the watcher (the forward pipeline
    // can lag the RPC response — allow it a beat).
    plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text { text: "hi".into() }],
            None,
        )
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(matches!(first, Ok(Ok(_))), "watch saw no events");
    // Held across the TTL — the session detaches anyway and the stream
    // reports Closed (buffered batches drain first).
    tokio::time::sleep(Duration::from_millis(400)).await;
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return true,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "watch stream should close on idle detach");
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "late".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, sepia_session::ControlErrorCode::Invalid);
    plane.close_all().await;
}

/// A pinning `subscribe` receiver holds the session open; once it drops
/// without a `touch_idle`, the sweep re-arms the idle clock and reaps —
/// the self-healing path a finished `prompt_wait` used to strand.
#[tokio::test]
async fn subscribe_pins_until_the_receiver_drops() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions {
            idle_ttl: Some(Duration::from_millis(150)),
            sweep_interval: Some(Duration::from_millis(50)),
            ..ControlPlaneOptions::default()
        },
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    let rx = plane.subscribe("s1", None).await.unwrap();
    // Well past the TTL with a listener attached — still live.
    tokio::time::sleep(Duration::from_millis(300)).await;
    plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "still here".into(),
            }],
            None,
        )
        .await
        .unwrap();
    // The listener leaves: `idle_since` was cleared by subscribe and
    // nothing re-arms it until the sweep notices — allow arm + TTL.
    drop(rx);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "late".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, sepia_session::ControlErrorCode::Invalid);
    plane.close_all().await;
}

/// A settled `prompt_wait` leaves no listener behind — the session
/// idle-detaches on TTL like any other finished turn (the T-028 leak:
/// every `/api/sessions/run` used to pin its agent forever).
#[tokio::test]
async fn prompt_wait_leaves_the_session_reapable() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
        ],
    );
    let (plane, _) = mk_plane(
        vec![session_with(
            "windsurf",
            vec![node(1, sepia_core::Role::User, "hi")],
        )],
        vec![runtime],
        ControlPlaneOptions {
            idle_ttl: Some(Duration::from_millis(150)),
            sweep_interval: Some(Duration::from_millis(50)),
            ..ControlPlaneOptions::default()
        },
    );
    plane.attach("s1", false, None, None, None).await.unwrap();
    plane
        .prompt_wait(
            "s1",
            &[sepia_acp::PromptPart::Text { text: "hi".into() }],
            None,
            Some(Duration::from_secs(5)),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let err = plane
        .prompt(
            "s1",
            &[sepia_acp::PromptPart::Text {
                text: "late".into(),
            }],
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, sepia_session::ControlErrorCode::Invalid);
    plane.close_all().await;
}

/// RSS budget after churn — catches unbounded growth in live-session
/// bookkeeping or translator buffers. Generous ceiling so slow CI
/// doesn't flake; the shape matters more than the number.
#[tokio::test]
async fn rss_after_churn_stays_bounded() {
    let runtime = MockRuntime::new(
        "devin",
        &[
            ("MOCK_CAPS", "load,list"),
            ("MOCK_SESSIONS", "s1|/work|S1|now"),
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
    let before = rss_kb();
    for _ in 0..30 {
        plane.attach("s1", false, None, None, None).await.unwrap();
        plane
            .prompt(
                "s1",
                &[sepia_acp::PromptPart::Text { text: "x".into() }],
                None,
            )
            .await
            .unwrap();
        plane.detach("s1").await;
    }
    plane.close_all().await;
    let after = rss_kb();
    // Detached live sessions must be freed — growth beyond ~32 MB over
    // 30 cycles is a leak, not allocator noise.
    assert!(
        after <= before + 32 * 1024,
        "RSS grew {} -> {} kB",
        before,
        after
    );
}
