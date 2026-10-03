import * as Cline from "./Cline.js";
import { Session } from "./Domain.js";

/**
 * The `sessions.db` index row and DDL in one place, so the CLI writes exactly
 * the shape the Cline CLI itself reads. This module stays runtime-agnostic
 * (node and bun) so the row shape is unit-testable without a database.
 */

/** Columns of the Cline CLI session index, in insert order. */
export const SESSION_COLUMNS = [
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
] as const;

export type SessionRow = Record<(typeof SESSION_COLUMNS)[number], string | number | null>;

/** DDL of the CLI's own index table, so installing into a fresh data dir works. */
export const SESSIONS_DDL = `CREATE TABLE IF NOT EXISTS sessions (
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
);`;

export const INSERT_SESSION = `INSERT OR REPLACE INTO sessions (${SESSION_COLUMNS.join(
  ", ",
)}) VALUES (${SESSION_COLUMNS.map(() => "?").join(", ")})`;

/**
 * Index row for an imported session. `pid: 0` and `status: "completed"` mark a
 * session that no live process owns; the CLI rewrites both when it resumes.
 */
export const sessionRow = (
  session: Session,
  sessionId: string,
  messagesPath: string,
  now: string = new Date().toISOString(),
): SessionRow => ({
  session_id: sessionId,
  source: "cli",
  pid: 0,
  started_at: new Date(session.createdAt * 1000).toISOString(),
  ended_at: new Date(session.lastActivityAt * 1000).toISOString(),
  exit_code: 0,
  status: "completed",
  status_lock: 0,
  interactive: 1,
  provider: Cline.CLINE_PROVIDER,
  model: session.model,
  cwd: session.workingDirectory,
  workspace_root: session.workingDirectory,
  team_name: null,
  enable_tools: 1,
  enable_spawn: 1,
  enable_teams: 1,
  parent_session_id: null,
  parent_agent_id: null,
  agent_id: null,
  conversation_id: null,
  is_subagent: 0,
  prompt: session.nodes.find((node) => node.role === "user")?.content ?? null,
  metadata_json: JSON.stringify({
    sessionHistoryOrigin: { mode: "user", version: Cline.CLINE_AGENT_VERSION },
    source: "cli",
    provider: Cline.CLINE_PROVIDER,
    model: session.model,
    title: session.title,
    enableTools: true,
    enableSpawn: true,
    enableTeams: true,
    interactive: true,
    mode: "act",
    importedFrom: { store: "devin", sessionId: session.id },
  }),
  transcript_path: "",
  hook_path: "",
  messages_path: messagesPath,
  updated_at: now,
});

/** True for a pid this process can still signal. */
export const isPidAlive = (pid: number): boolean => {
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error instanceof Error && "code" in error && error.code === "EPERM";
  }
};

/** A session row still belongs to a live owner: never silently replace it. */
export const isActiveRow = (row: { status: string; pid: number }, pidAlive: boolean): boolean =>
  new Set(["running", "idle", "pending"]).has(row.status) && pidAlive;
