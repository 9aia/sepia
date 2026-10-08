#![allow(clippy::unwrap_used, clippy::pedantic)]

use sepia_outbox::{OpKind, Outbox, Status};
use serde_json::json;

#[test]
fn enqueue_is_idempotent_on_key() {
    let outbox = Outbox::in_memory().unwrap();
    let a = outbox
        .enqueue(
            "n1",
            "s1",
            "prompt",
            OpKind::Turn,
            json!({"t": "hi"}),
            "k1",
            None,
        )
        .unwrap();
    let b = outbox
        .enqueue(
            "n1",
            "s1",
            "prompt",
            OpKind::Turn,
            json!({"t": "hi"}),
            "k1",
            None,
        )
        .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(outbox.pending_for_node("n1").unwrap().len(), 1);
}

#[test]
fn per_session_fifo() {
    let outbox = Outbox::in_memory().unwrap();
    outbox
        .enqueue("n1", "s1", "prompt", OpKind::Turn, json!(1), "a", None)
        .unwrap();
    outbox
        .enqueue("n1", "s1", "cancel", OpKind::Turn, json!(2), "b", None)
        .unwrap();
    outbox
        .enqueue("n1", "s2", "prompt", OpKind::Turn, json!(3), "c", None)
        .unwrap();
    assert_eq!(outbox.peek("n1", "s1").unwrap().unwrap().op, "prompt");
    assert_eq!(outbox.peek("n1", "s2").unwrap().unwrap().op, "prompt");
    assert!(outbox.peek("n1", "s3").unwrap().is_none());
}

#[test]
fn turn_ops_dead_letter_on_first_failure() {
    let outbox = Outbox::in_memory().unwrap();
    let e = outbox
        .enqueue("n1", "s1", "prompt", OpKind::Turn, json!(null), "k", None)
        .unwrap();
    outbox.mark_in_flight(&e.id).unwrap();
    assert_eq!(
        outbox.mark_failed(&e.id, "node unreachable").unwrap(),
        Status::Dead
    );
    assert!(outbox.peek("n1", "s1").unwrap().is_none());
    assert_eq!(outbox.dead_letters().unwrap().len(), 1);
    // Resurrect for another attempt.
    outbox.retry_dead(&e.id).unwrap();
    assert_eq!(outbox.peek("n1", "s1").unwrap().unwrap().op, "prompt");
}

#[test]
fn metadata_ops_retry_until_budget() {
    let outbox = Outbox::in_memory().unwrap();
    let e = outbox
        .enqueue(
            "n1",
            "s1",
            "meta.patch",
            OpKind::Metadata,
            json!({}),
            "k",
            None,
        )
        .unwrap();
    for _ in 0..4 {
        outbox.mark_in_flight(&e.id).unwrap();
        assert_eq!(outbox.mark_failed(&e.id, "down").unwrap(), Status::Pending);
    }
    outbox.mark_in_flight(&e.id).unwrap();
    assert_eq!(outbox.mark_failed(&e.id, "down").unwrap(), Status::Dead);
}

#[test]
fn ttl_expiry_dead_letters() {
    let outbox = Outbox::in_memory().unwrap();
    outbox
        .enqueue(
            "n1",
            "s1",
            "prompt",
            OpKind::Turn,
            json!(null),
            "k",
            Some(-1),
        )
        .unwrap();
    // Expired at enqueue+(-1s) → already past due on next peek.
    assert!(outbox.peek("n1", "s1").unwrap().is_none());
    assert_eq!(
        outbox.dead_letters().unwrap()[0].last_error.as_deref(),
        Some("expired")
    );
}

#[test]
fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("outbox.db");
    {
        let outbox = Outbox::open(&path).unwrap();
        outbox
            .enqueue(
                "n1",
                "s1",
                "prompt",
                OpKind::Turn,
                json!({"t": "hi"}),
                "k1",
                None,
            )
            .unwrap();
    }
    let outbox = Outbox::open(&path).unwrap();
    let pending = outbox.pending_for_node("n1").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload, json!({"t": "hi"}));
}
