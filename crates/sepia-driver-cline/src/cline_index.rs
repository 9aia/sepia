//! `ClineIndex.ts` port — the `sessions.db` index row and DDL in one place,
//! so the driver writes exactly the shape the Cline CLI itself reads.

use sepia_core::domain::Session;
use serde_json::{Map, Value, json};

use crate::cline;

/// Columns of the Cline CLI session index, in insert order.
pub const SESSION_COLUMNS: [&str; 28] = [
    "session_id",
    "source",
    "pid",
    "started_at",
    "ended_at",
    "exit_code",
    "status",
    "status_lock",
    "interactive",
    "provider",
    "model",
    "cwd",
    "workspace_root",
    "team_name",
    "enable_tools",
    "enable_spawn",
    "enable_teams",
    "parent_session_id",
    "parent_agent_id",
    "agent_id",
    "conversation_id",
    "is_subagent",
    "prompt",
    "metadata_json",
    "transcript_path",
    "hook_path",
    "messages_path",
    "updated_at",
];

/// DDL of the CLI's own index table, so installing into a fresh data dir works.
pub const SESSIONS_DDL: &str = "CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  pid INTEGER NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  exit_code INTEGER,
  status TEXT NOT NULL,
  status_lock INTEGER NOT NULL DEFAULT 0,
  interactive INTEGER NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  cwd TEXT NOT NULL,
  workspace_root TEXT NOT NULL,
  team_name TEXT,
  enable_tools INTEGER NOT NULL,
  enable_spawn INTEGER NOT NULL,
  enable_teams INTEGER NOT NULL,
  parent_session_id TEXT,
  parent_agent_id TEXT,
  agent_id TEXT,
  conversation_id TEXT,
  is_subagent INTEGER NOT NULL DEFAULT 0,
  prompt TEXT,
  metadata_json TEXT,
  transcript_path TEXT NOT NULL DEFAULT '',
  hook_path TEXT NOT NULL,
  messages_path TEXT,
  updated_at TEXT NOT NULL
);";

/// `INSERT OR REPLACE INTO sessions (...28 columns...) VALUES (?, …)` —
/// placeholders are generated from the column list so the two cannot drift.
#[must_use]
pub fn insert_session_sql() -> String {
    let marks = SESSION_COLUMNS
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT OR REPLACE INTO sessions ({}) VALUES ({marks})",
        SESSION_COLUMNS.join(", ")
    )
}

/// Index row for an imported session, keyed in column order. `pid: 0` and
/// `status: "completed"` mark a session that no live process owns; the CLI
/// rewrites both when it resumes. `now` is the `updated_at` ISO stamp;
/// `None` stamps the current time.
pub fn session_row(
    session: &Session,
    session_id: &str,
    messages_path: &str,
    now: &str,
) -> Map<String, Value> {
    let mut row = Map::new();
    let mut put = |key: &str, value: Value| {
        row.insert(key.into(), value);
    };
    put("session_id", json!(session_id));
    put("source", json!("cli"));
    put("pid", json!(0));
    put("started_at", json!(cline::to_iso_ms(session.created_at)));
    put(
        "ended_at",
        json!(cline::to_iso_ms(session.last_activity_at)),
    );
    put("exit_code", json!(0));
    put("status", json!("completed"));
    put("status_lock", json!(0));
    put("interactive", json!(1));
    put("provider", json!(cline::CLINE_PROVIDER));
    put("model", json!(session.model));
    put("cwd", json!(session.working_directory));
    put("workspace_root", json!(session.working_directory));
    put("team_name", Value::Null);
    put("enable_tools", json!(1));
    put("enable_spawn", json!(1));
    put("enable_teams", json!(1));
    put(
        "parent_session_id",
        session
            .parent_session_id
            .as_ref()
            .map_or(Value::Null, |v| json!(v)),
    );
    put("parent_agent_id", Value::Null);
    put(
        "agent_id",
        session.agent_id.as_ref().map_or(Value::Null, |v| json!(v)),
    );
    put("conversation_id", Value::Null);
    put(
        "is_subagent",
        json!(i32::from(session.parent_session_id.is_some())),
    );
    put(
        "prompt",
        session
            .nodes
            .iter()
            .find(|node| node.role == sepia_core::domain::Role::User)
            .map_or(Value::Null, |n| json!(n.content)),
    );
    put(
        "metadata_json",
        json!(
            json!({
                "sessionHistoryOrigin": { "mode": "user", "version": cline::CLINE_AGENT_VERSION },
                "source": "cli",
                "provider": cline::CLINE_PROVIDER,
                "model": session.model,
                "title": session.title,
                "enableTools": true,
                "enableSpawn": true,
                "enableTeams": true,
                "interactive": true,
                "mode": "act",
                "importedFrom": { "store": "devin", "sessionId": session.id },
            })
            .to_string()
        ),
    );
    put("transcript_path", json!(""));
    put("hook_path", json!(""));
    put("messages_path", json!(messages_path));
    put("updated_at", json!(now));
    row
}

/// True for a pid this process can still signal. `/proc/<pid>` standing in
/// for `process.kill(pid, 0)` — on Linux an existing entry means the
/// process exists (including one owned by another user, matching the
/// `EPERM` branch of the TS).
pub fn is_pid_alive(pid: i64) -> bool {
    if pid <= 0 || pid > 9_007_199_254_740_991 {
        return false;
    }
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// A session row still belongs to a live owner: never silently replace it.
pub fn is_active_row(status: &str, pid_alive: bool) -> bool {
    matches!(status, "running" | "idle" | "pending") && pid_alive
}
