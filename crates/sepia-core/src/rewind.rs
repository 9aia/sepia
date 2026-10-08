//! Conversation rewind planning — pure math over `Session::nodes`,
//! separated from store-specific writers so the same cut is unit testable
//! and every store truncates identically.
//!
//! A rewind is a prefix cut: `nodes[0..keep_count)` survive, everything
//! after is gone.

use std::collections::BTreeSet;

use crate::domain::{MessageNode, Role, Session};

/// Selector for the cut point — exactly one member is set.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewindTarget {
    /// Keep this node and everything before it.
    NodeId(i64),
    /// Drop the last N user turns.
    Turns(i64),
    /// Keep up to the last node recorded at or before this checkpoint ref.
    Checkpoint(String),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindPlan {
    /// `nodes[0..keep_count)` survive.
    pub keep_count: usize,
    pub kept: Vec<MessageNode>,
    pub removed: Vec<MessageNode>,
    /// Tool-call ids that exist only inside removed nodes — the rows a
    /// `tool_call_state`-style store cleans alongside the messages.
    pub removed_tool_call_ids: Vec<String>,
}

/// Resolve a `RewindTarget` to a prefix cut of `session.nodes`.
///
/// `removed` is empty when the target is already the tail — callers treat
/// that as a no-op rather than writing the store.
pub fn plan_rewind(session: &Session, target: &RewindTarget) -> Result<RewindPlan, String> {
    let nodes = &session.nodes;

    let keep_count = match target {
        RewindTarget::NodeId(node_id) => {
            let index = nodes
                .iter()
                .position(|node| node.node_id == *node_id)
                .ok_or_else(|| format!("Unknown node: {node_id}"))?;
            index + 1
        }
        RewindTarget::Turns(turns) => {
            if *turns < 1 {
                return Err("turns must be a positive integer".into());
            }
            let user_indices: Vec<usize> = nodes
                .iter()
                .enumerate()
                .filter_map(|(index, node)| (node.role == Role::User).then_some(index))
                .collect();
            if user_indices.is_empty() {
                return Err("the session has no user turns to drop".into());
            }
            let from_end = user_indices.len().saturating_sub(*turns as usize);
            user_indices.get(from_end).copied().unwrap_or(0)
        }
        RewindTarget::Checkpoint(checkpoint) => {
            let entry = session
                .checkpoints
                .iter()
                .find(|candidate| candidate.r#ref == *checkpoint)
                .ok_or_else(|| format!("Unknown checkpoint ref: {checkpoint}"))?;
            let mut keep = 0;
            for (index, node) in nodes.iter().enumerate() {
                if node.created_at * 1000.0 <= entry.created_at {
                    keep = index + 1;
                }
            }
            keep
        }
    };

    let kept = nodes[..keep_count].to_vec();
    let removed = nodes[keep_count..].to_vec();
    let mut removed_ids = BTreeSet::new();
    for node in &removed {
        for call in &node.tool_calls {
            removed_ids.insert(call.id.clone());
        }
        if let Some(answered) = &node.tool_call_id {
            removed_ids.insert(answered.clone());
        }
    }
    let mut surviving_ids = BTreeSet::new();
    for node in &kept {
        for call in &node.tool_calls {
            surviving_ids.insert(call.id.clone());
        }
    }
    Ok(RewindPlan {
        keep_count,
        kept,
        removed,
        removed_tool_call_ids: removed_ids
            .into_iter()
            .filter(|id| !surviving_ids.contains(id))
            .collect(),
    })
}

/// `session` with the plan's cut applied — the form save-based writers
/// persist. `last_activity_at`/`main_chain_id` move back to the surviving tail.
pub fn rewind_session(session: &Session, plan: &RewindPlan) -> Session {
    let tail = plan.kept.last();
    Session {
        id: session.id.clone(),
        title: session.title.clone(),
        working_directory: session.working_directory.clone(),
        backend_type: session.backend_type.clone(),
        agent_mode: session.agent_mode.clone(),
        model: session.model.clone(),
        created_at: session.created_at,
        last_activity_at: tail.map_or(session.created_at, |t| t.created_at),
        main_chain_id: tail.map_or(0, |t| t.node_id),
        shell_last_seen_index: session.shell_last_seen_index,
        cogs_json: session.cogs_json.clone(),
        workspace_dirs: session.workspace_dirs.clone(),
        hidden: session.hidden,
        parent_session_id: session.parent_session_id.clone(),
        agent_id: session.agent_id.clone(),
        checkpoints: session.checkpoints.clone(),
        metadata: session.metadata.clone(),
        nodes: plan.kept.clone(),
        prompt_history: session.prompt_history.clone(),
    }
}
