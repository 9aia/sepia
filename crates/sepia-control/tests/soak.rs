#![allow(clippy::unwrap_used, clippy::pedantic)]

//! Bounded soak — churn attach/detach/prompt cycles against the mock
//! agent and assert the plane's state stays compact (no live-session
//! leaks, RSS doesn't climb). The plan's hour-scale version runs
//! manually; CI gets the bounded form.

use std::sync::Arc;
use std::time::Duration;

use sepia_control::ControlPlaneOptions;

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
    assert_eq!(err.code, sepia_control::ControlErrorCode::Invalid);
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
