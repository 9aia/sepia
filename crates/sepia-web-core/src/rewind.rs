//! Per-message rewind — the UI's "Rewind to this message" action.
//!
//! The node's `POST /api/sessions/{id}/rewind` takes `{confirm}` plus
//! exactly one selector: `nodeId` keeps that transcript node and
//! everything before it (the per-message target), `checkpoint` rewinds
//! to a recorded snapshot ref. A missing selector capability surfaces
//! on the summary row as `canRewind`, so this module only shapes the
//! request/label — the store itself decides whether the cut lands.

use serde::{Deserialize, Serialize};

/// The `rewind` selector — serializes into the wire field names the
/// node expects (`{"nodeId": …}` / `{"checkpoint": …}`), so it merges
/// into the POST body verbatim next to `{"confirm": true}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewindSelector {
    /// Keep this transcript node and everything before it — the
    /// per-message target the hover strip passes.
    NodeId(i64),
    /// Keep up to the last node recorded at/before this checkpoint ref.
    Checkpoint(String),
}

/// Whether a history row can anchor a per-message rewind: user and
/// assistant rows only (tool/system rows carry no action strip), and
/// only when the wire carried the row's `nodeId` — that id *is* the
/// cut point, so a tolerant-decode fallback must never stand in for
/// it (a guessed id could truncate the whole session).
pub fn rewind_node_id(role: &str, node_id: Option<i64>) -> Option<i64> {
    if matches!(role, "user" | "assistant") {
        node_id.filter(|id| *id >= 0)
    } else {
        None
    }
}

/// The toast line a rewind result maps to — `removed == 0` is the
/// no-op answer for "already at that point".
pub fn rewind_result_summary(removed: i64) -> String {
    if removed <= 0 {
        "Nothing to rewind — already at that point".to_string()
    } else {
        format!(
            "Rewound {removed} message{}",
            if removed == 1 { "" } else { "s" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_serializes_to_wire_field_names() {
        assert_eq!(
            serde_json::to_value(RewindSelector::NodeId(7)).unwrap(),
            serde_json::json!({ "nodeId": 7 })
        );
        assert_eq!(
            serde_json::to_value(RewindSelector::Checkpoint("cp-1".into())).unwrap(),
            serde_json::json!({ "checkpoint": "cp-1" })
        );
        // And merges into the request body next to `confirm`.
        let mut body = serde_json::json!({ "confirm": true });
        body.as_object_mut().unwrap().extend(
            serde_json::to_value(RewindSelector::NodeId(3))
                .unwrap()
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(body, serde_json::json!({ "confirm": true, "nodeId": 3 }));
        // Tolerant decode — server fns deserialize the same shape.
        let parsed: RewindSelector =
            serde_json::from_value(serde_json::json!({ "nodeId": 9 })).unwrap();
        assert_eq!(parsed, RewindSelector::NodeId(9));
    }

    #[test]
    fn rewind_node_id_gates_on_role_and_present_id() {
        assert_eq!(rewind_node_id("user", Some(4)), Some(4));
        assert_eq!(rewind_node_id("assistant", Some(0)), Some(0));
        assert_eq!(rewind_node_id("tool", Some(4)), None);
        assert_eq!(rewind_node_id("system", Some(4)), None);
        // No node id on the wire → no target, never a guess.
        assert_eq!(rewind_node_id("user", None), None);
        assert_eq!(rewind_node_id("assistant", Some(-1)), None);
    }

    #[test]
    fn result_summary_names_the_noop_and_the_cut() {
        assert_eq!(
            rewind_result_summary(0),
            "Nothing to rewind — already at that point"
        );
        assert_eq!(rewind_result_summary(-2), rewind_result_summary(0));
        assert_eq!(rewind_result_summary(1), "Rewound 1 message");
        assert_eq!(rewind_result_summary(12), "Rewound 12 messages");
    }
}
