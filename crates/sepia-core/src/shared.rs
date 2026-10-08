//! Shared domain services every adapter may use — outcome folding,
//! canonical session defaults, checkpoint-metadata convention,
//! project-dir slugs. Lives in core so no adapter imports another adapter.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::domain::{CheckpointRef, MessageNode, ToolCall, ToolCallStatus};

/// The outcome a store recorded for one tool call, folded back onto the call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallOutcome {
    pub status: ToolCallStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<f64>,
}

/// Outcomes keyed by tool-call id, collected from `role: "tool"` nodes.
pub fn tool_node_outcomes(nodes: &[MessageNode]) -> HashMap<String, ToolCallOutcome> {
    let mut outcomes = HashMap::new();
    for node in nodes {
        let (Some(call_id), Some(result)) = (&node.tool_call_id, &node.tool_result) else {
            continue;
        };
        if node.role != crate::domain::Role::Tool {
            continue;
        }
        outcomes.insert(
            call_id.clone(),
            ToolCallOutcome {
                status: result.status,
                exit_code: result.exit_code,
                duration_ms: result.duration_ms,
            },
        );
    }
    outcomes
}

fn with_outcome(tc: &ToolCall, outcome: &ToolCallOutcome) -> ToolCall {
    let mut next = tc.clone();
    next.status = Some(outcome.status);
    if let Some(exit_code) = outcome.exit_code {
        next.exit_code = Some(exit_code);
    }
    if let Some(duration_ms) = outcome.duration_ms {
        next.duration_ms = Some(duration_ms);
    }
    next
}

/// The result a store keeps on `role: "tool"` messages is the authoritative
/// outcome; fold it onto the `ToolCall` objects that produced it.
pub fn apply_tool_call_outcomes<S: std::hash::BuildHasher>(
    nodes: &[MessageNode],
    outcomes: &HashMap<String, ToolCallOutcome, S>,
) -> Vec<MessageNode> {
    if outcomes.is_empty() {
        return nodes.to_vec();
    }
    nodes
        .iter()
        .map(|node| {
            if node
                .tool_calls
                .iter()
                .all(|tc| !outcomes.contains_key(&tc.id))
            {
                return node.clone();
            }
            let mut next = node.clone();
            next.tool_calls = node
                .tool_calls
                .iter()
                .map(|tc| match outcomes.get(&tc.id) {
                    Some(outcome) => with_outcome(tc, outcome),
                    None => tc.clone(),
                })
                .collect();
            next
        })
        .collect()
}

pub fn default_session_metadata() -> Value {
    json!({
        "total_credit_cost": 0,
        "total_acu_cost": 0,
        "response_dimensions": [
            {
                "uid": "agent_messages",
                "group_title": "Response Statistics",
                "kind": {
                    "CumulativeMetric": {
                        "label": "Agent messages",
                        "value": 0,
                        "tail": " message",
                        "plural_tail": " messages",
                        "prefix": ""
                    }
                }
            },
            {
                "uid": "model",
                "group_title": "Response Statistics",
                "kind": { "Metric": { "label": "Model", "value": "Imported from Cline" } }
            }
        ]
    })
}

pub fn default_cogs_json() -> String {
    json!([
        {
            "source": { "Session": "User" },
            "lifetime": { "Unique": "core/plan_mask" },
            "set_system_prefix": null,
            "append_system_messages": [],
            "context": [],
            "footer_messages": [],
            "user_display": [],
            "permissions": [],
            "tool_availability": null,
            "model": null
        },
        {
            "source": { "Session": "User" },
            "lifetime": { "Unique": "core/accept_edits" },
            "set_system_prefix": null,
            "append_system_messages": [],
            "context": [],
            "footer_messages": [],
            "user_display": [],
            "permissions": [],
            "tool_availability": null,
            "model": null
        },
        {
            "source": { "Session": "System" },
            "lifetime": { "Unique": "core/parallel-tool-calls" },
            "set_system_prefix": null,
            "append_system_messages": [],
            "context": [],
            "footer_messages": [],
            "user_display": [],
            "permissions": [],
            "tool_availability": null,
            "model": null
        },
        {
            "source": { "Session": "System" },
            "lifetime": { "Unique": "core/model" },
            "set_system_prefix": null,
            "append_system_messages": [],
            "context": [],
            "footer_messages": [],
            "user_display": [],
            "permissions": [],
            "tool_availability": null,
            "model": null
        }
    ])
    .to_string()
}

/// The namespaced slot sepia keeps workspace-snapshot refs under in the
/// `sessions.metadata` JSON.
pub const SESSION_CHECKPOINTS_KEY: &str = "sepia/checkpoints";

fn finite_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

/// Tolerant read of a `sepia/checkpoints` metadata value: entries that are
/// not `{ref, createdAt}` objects are dropped rather than failing the row.
pub fn checkpoints_from_metadata(metadata: &Value) -> Vec<CheckpointRef> {
    let Some(raw) = metadata.get(SESSION_CHECKPOINTS_KEY) else {
        return Vec::new();
    };
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let ref_ = item.get("ref")?.as_str()?.to_string();
            let created_at = finite_number(item.get("createdAt")?)?;
            Some(CheckpointRef {
                r#ref: ref_,
                created_at,
                run_count: item.get("runCount").and_then(Value::as_i64),
                kind: item.get("kind").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

/// Decode a project dir name back to a path — lossy (`-home-me-proj` →
/// `/home/me/proj`). Used only when no entry in the file carries a `cwd`.
pub fn decode_project_dir(name: &str) -> String {
    let decoded = name.replace('-', "/");
    if decoded.starts_with('/') {
        decoded
    } else {
        format!("/{decoded}")
    }
}

/// The inverse of `decode_project_dir` (`/home/me/proj` → `-home-me-proj`).
/// Lossy: `my proj` and `my-proj` collide.
pub fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
