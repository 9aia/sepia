export type AgentKind = "devin" | "cline" | "claude" | "cursor";

/**
 * One run span of a session — which agent on which node continued it. The
 * owning server's meta overlay appends a span per attach; `node` is that
 * server's real node id (never the `"local"` alias used in row keys).
 */
export interface RunSpan {
  /** Epoch milliseconds when the span was recorded (attach time). */
  at: number;
  agent: string;
  node: string;
}

export interface SessionSummary {
  id: string;
  title: string;
  cwd: string;
  agent: AgentKind;
  /**
   * Owning node id, present once peers are registered: `"local"` for this
   * machine, the peer's id otherwise. Absent in single-node mode so keys and
   * URLs stay `agent:id` exactly as before (docs/protocol.md).
   */
  node?: string;
  updatedAt: string;
  locked: boolean;
  lockHolderPid: number | null;
  source: string;
  busy: boolean;
  pinned: boolean;
  archived: boolean;
  projectIds: string[];
  model: string | null;
  /** Run provenance — which agent ran the session on which node, per attach. */
  spans: RunSpan[];
  /** Id of the session that spawned this one, when the store records a sub-agent tree. */
  parentSessionId?: string;
  /** Sub-agent identity within the parent's team; not the agent runtime. */
  agentId?: string;
}

export interface Project {
  id: string;
  name: string;
  /** Owning node id — see SessionSummary.node. */
  node?: string;
}

/** GET /api/node — a federated node's self-description. */
export interface NodeDescriptor {
  id: string;
  name: string;
  version: string;
  protocol: number;
  agents: string[];
  capabilities: string[];
}

export interface AgentInfo {
  id: string;
  label: string;
}

export interface CreateSessionInput {
  cwd: string;
  agent?: string;
  title?: string;
  model?: string;
  fallbacks?: readonly string[];
}

export type HistoryRole = "user" | "assistant" | "tool" | "system";

/** Token metrics an agent's store recorded for one message. */
export interface MessageUsage {
  input: number;
  output: number;
  cacheRead?: number;
  cacheWrite?: number;
  thinking?: number;
  cost?: number;
}

export type ToolCallStatus = "pending" | "success" | "error";

/**
 * One content block of a history message — the wire form of the IR `Block`
 * union (sepia-core Domain.ts). Present only when the agent's store recorded
 * non-text content; `content` remains the joined text projection.
 */
export type HistoryBlock =
  | { type: "text"; text: string }
  | { type: "image"; data?: string; mimeType?: string; uri?: string }
  | { type: "audio"; data?: string; mimeType?: string }
  | {
      type: "file";
      uri?: string;
      name?: string;
      mimeType?: string;
      size?: number;
      text?: string;
      data?: string;
    };

export interface HistoryMessage {
  role: HistoryRole;
  content: string;
  blocks?: HistoryBlock[];
  createdAt: number;
  toolName?: string;
  usage?: MessageUsage;
  model?: string;
  requestId?: string;
  finishReason?: string;
  /** Tool-result messages only: how the call this message answers ended. */
  toolStatus?: ToolCallStatus;
  exitCode?: number;
  durationMs?: number;
}

export interface HistoryPage {
  messages: HistoryMessage[];
  total: number;
  start: number;
}

export interface AttachResult {
  attached: boolean;
  readOnly: boolean;
  /** The agent the session attached under (server ≥ provenance spans). */
  agentId?: string;
}

export interface PermissionRequest {
  requestId: string;
  title: string;
  options: PermissionOption[];
}

export interface PermissionOption {
  optionId: string;
  label: string;
  kind?: string;
}

export interface UserInfo {
  username: string;
  homedir: string;
  shell: string | null;
  hostname: string;
  platform: string;
  arch: string;
}
