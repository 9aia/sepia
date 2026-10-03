import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const sessions = sqliteTable("sessions", {
  id: text("id").primaryKey(),
  workingDirectory: text("working_directory").notNull(),
  backendType: text("backend_type").notNull(),
  model: text("model").notNull(),
  agentMode: text("agent_mode").notNull(),
  createdAt: integer("created_at").notNull(),
  lastActivityAt: integer("last_activity_at").notNull(),
  title: text("title").notNull(),
  mainChainId: integer("main_chain_id").notNull(),
  shellLastSeenIndex: integer("shell_last_seen_index").notNull().default(0),
  cogsJson: text("cogs_json").notNull().default("[]"),
  workspaceDirs: text("workspace_dirs").notNull().default("[]"),
  hidden: integer("hidden").notNull().default(0),
  metadata: text("metadata").notNull().default("{}"),
});

export const messageNodes = sqliteTable("message_nodes", {
  rowId: integer("row_id").primaryKey({ autoIncrement: true }),
  sessionId: text("session_id")
    .notNull()
    .references(() => sessions.id),
  nodeId: integer("node_id").notNull(),
  parentNodeId: integer("parent_node_id"),
  chatMessage: text("chat_message").notNull(),
  createdAt: integer("created_at").notNull(),
  metadata: text("metadata"),
});

export const promptHistory = sqliteTable("prompt_history", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  content: text("content").notNull(),
  timestamp: integer("timestamp").notNull(),
  sessionId: text("session_id")
    .notNull()
    .references(() => sessions.id),
  isShell: integer("is_shell").notNull().default(0),
});
