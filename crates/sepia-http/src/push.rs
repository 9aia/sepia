//! Web Push wiring for the session plane — the `PushBackend`
//! implementation that keeps VAPID keys + subscriptions inside the
//! KV's internal `push` namespace plus the session-event →
//! notification mapping. The push *machinery* (VAPID, subscription
//! store, send) is `hypha_push`; what to notify and when stays
//! sepia's.

use std::collections::BTreeMap;

use hypha_kv::KvStore;
use hypha_push::PushStore;
use sepia_proto::SessionEvent;
use serde_json::Value;

/// The node's push persistence — the internal `push` KV namespace
/// (`push/vapid`, `push/pushSubscriptions`), so secrets ride the same
/// store-API-only visibility `/api/kv*` enforces for internal
/// namespaces. A newtype because both `KvStore` and `PushBackend` are
/// foreign to this crate's push surface (orphan rule) — the wrapper
/// is the local type the impl hangs on.
#[derive(Clone)]
pub struct KvPushBackend(pub std::sync::Arc<dyn KvStore>);

impl hypha_push::PushBackend for KvPushBackend {
    fn config(&self) -> BTreeMap<String, Value> {
        self.0.list(crate::kv::NS_PUSH)
    }

    fn set_config(&self, key: &str, value: Value) {
        self.0.set(crate::kv::NS_PUSH, key, value);
    }
}

/// Session-event → notification mapping — run completion and permission
/// requests, matching the TS `notifyForEvents`.
pub fn notify_for_events(
    push: &PushStore,
    session_id: &str,
    agent: Option<&str>,
    session_title: &str,
    events: &[SessionEvent],
) {
    let key = agent.map_or_else(|| session_id.to_string(), |a| format!("{a}:{session_id}"));
    let url = format!("/?session={}", urlencoding(&key));
    if events
        .iter()
        .any(|e| matches!(e, SessionEvent::RunFinished { .. }))
    {
        push.send(
            hypha_push::Kind::Done,
            "Session finished",
            &format!("{session_title} finished its run."),
            &url,
        );
    }
    let wants_permission = events.iter().any(
        |e| matches!(e, SessionEvent::Custom { name, .. } if name == "acp:permission_request"),
    );
    if wants_permission {
        push.send(
            hypha_push::Kind::Permission,
            "Approval needed",
            &format!("{session_title} is waiting for you."),
            &url,
        );
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn urlencoding_escapes() {
        assert_eq!(urlencoding("devin:s1"), "devin%3As1");
        assert_eq!(urlencoding("plain-_.~"), "plain-_.~");
    }
}
