//! `session.rewind` for the Devin store — a second, writable sqlite
//! connection deletes the suffix rows in one transaction (the store the
//! driver serves is read-only; the rewinder is the sanctioned writer).

use std::path::PathBuf;

use async_trait::async_trait;
use rusqlite::Connection;
use sepia_core::Session;
use sepia_core::rewind::RewindPlan;
use sepia_driver_sdk::SessionTruncator;

pub struct DevinTruncator {
    db_path: PathBuf,
    has_tool_call_state: bool,
    has_subagent_heads: bool,
}

impl DevinTruncator {
    pub fn new(db_path: PathBuf, has_tool_call_state: bool, has_subagent_heads: bool) -> Self {
        Self {
            db_path,
            has_tool_call_state,
            has_subagent_heads,
        }
    }
}

#[async_trait]
impl SessionTruncator for DevinTruncator {
    async fn truncate(
        &self,
        session: &Session,
        plan: &RewindPlan,
        truncated: &Session,
    ) -> Result<(), String> {
        let conn = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| e.to_string())?;
        let removed_node_ids: Vec<i64> = plan.removed.iter().map(|n| n.node_id).collect();
        conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
        let run = || -> Result<(), rusqlite::Error> {
            if !removed_node_ids.is_empty() {
                let marks = removed_node_ids
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut stmt = conn.prepare(&format!(
                    "DELETE FROM message_nodes WHERE session_id = ? AND node_id IN ({marks})"
                ))?;
                let mut idx = 1;
                stmt.raw_bind_parameter(idx, &session.id)?;
                idx += 1;
                for id in &removed_node_ids {
                    stmt.raw_bind_parameter(idx, id)?;
                    idx += 1;
                }
                stmt.raw_execute()?;
                if self.has_subagent_heads {
                    // `chain_node_id` is the parent node that spawned the
                    // subagent — a spawn in a removed turn leaves a stale link.
                    let mut stmt = conn.prepare(&format!(
                        "DELETE FROM subagent_heads WHERE session_id = ? AND chain_node_id IN ({marks})"
                    ))?;
                    let mut idx = 1;
                    stmt.raw_bind_parameter(idx, &session.id)?;
                    idx += 1;
                    for id in &removed_node_ids {
                        stmt.raw_bind_parameter(idx, id)?;
                        idx += 1;
                    }
                    stmt.raw_execute()?;
                }
            }
            if self.has_tool_call_state && !plan.removed_tool_call_ids.is_empty() {
                let marks = plan
                    .removed_tool_call_ids
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut stmt = conn.prepare(&format!(
                    "DELETE FROM tool_call_state WHERE session_id = ? AND tool_call_id IN ({marks})"
                ))?;
                let mut idx = 1;
                stmt.raw_bind_parameter(idx, &session.id)?;
                idx += 1;
                for id in &plan.removed_tool_call_ids {
                    stmt.raw_bind_parameter(idx, id)?;
                    idx += 1;
                }
                stmt.raw_execute()?;
            }
            conn.execute(
                "UPDATE sessions SET last_activity_at = ?, main_chain_id = ? WHERE id = ?",
                rusqlite::params![
                    truncated.last_activity_at,
                    truncated.main_chain_id,
                    session.id
                ],
            )?;
            Ok(())
        };
        match run() {
            Ok(()) => conn
                .execute_batch("COMMIT")
                .map_err(|e| format!("Failed to truncate session: {e}")),
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(format!("Failed to truncate session: {e}"))
            }
        }
    }
}
