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

/**
 * ACP `agentCapabilities.promptCapabilities` — which prompt content blocks
 * beyond the baseline (text + resource links) the agent accepts. Flags not
 * advertised by the agent come back false.
 */
export interface PromptCapabilities {
  image: boolean;
  audio: boolean;
  embeddedContext: boolean;
}

/** ACP `agentCapabilities.sessionCapabilities` flattened to booleans. */
export interface AgentSessionCapabilities {
  list: boolean;
  delete: boolean;
  fork: boolean;
  resume: boolean;
  close: boolean;
  additionalDirectories: boolean;
}

/**
 * The ACP `initialize` capability advertisement a node probed from an
 * agent. Present on attach/create results and on `/api/agents` entries
 * once that agent has been spawned at least once.
 */
export interface AgentCapabilities {
  loadSession: boolean;
  sessionList: boolean;
  promptCapabilities: PromptCapabilities;
  sessionCapabilities: AgentSessionCapabilities;
}

export interface AgentInfo {
  id: string;
  label: string;
  /** Probed capabilities — absent until the node's first spawn of this agent. */
  capabilities?: AgentCapabilities;
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

/** A file a tool call touched — the ACP `locations` entries stores record. */
export interface ToolLocation {
  path: string;
  line?: number;
}

/**
 * The before/after payload a store recorded for one file change — Devin's
 * ACP `diff` content, Cline `editor` inputs. Absent `oldText` = create,
 * absent `newText` = delete.
 */
export interface ToolFileDiff {
  path: string;
  oldText?: string;
  newText?: string;
}

/**
 * A non-diff `content` entry of a live tool call — the ACP `ToolCallContent`
 * kinds beyond `diff`: a terminal ref (`terminalId`, `output` only when the
 * agent inlined the text) or an embedded content block (text/image).
 */
export type ToolCallContent =
  | { type: "terminal"; terminalId: string; output?: string }
  | { type: "text"; text: string }
  | { type: "image"; data?: string; uri?: string; mimeType?: string };

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
  /** The node this row came from — a `nodeId` rewind truncates after it. */
  nodeId?: number;
  content: string;
  blocks?: HistoryBlock[];
  createdAt: number;
  toolName?: string;
  /** Reasoning text the store recorded (`"[redacted]"` marks an opaque block). */
  thinking?: string;
  /** Opaque provider seal on `thinking` — preserved verbatim for resume. */
  thinkingSignature?: string;
  usage?: MessageUsage;
  model?: string;
  requestId?: string;
  finishReason?: string;
  /** Tool-result messages only: how the call this message answers ended. */
  toolStatus?: ToolCallStatus;
  exitCode?: number;
  durationMs?: number;
  /**
   * Tool-result messages only: the call's raw input args, JSON-encoded —
   * joined from the IR `ToolCall.arguments`, same shape as a live row's
   * `args` stream (a single JSON value rather than concatenated snapshots).
   */
  args?: string;
  /** Tool-result messages only: files the call touched / changed. */
  locations?: ToolLocation[];
  diffs?: ToolFileDiff[];
  /** Tool-result messages only: the call this row answers — a restore reverts against it. */
  toolCallId?: string;
}

export interface HistoryPage {
  messages: HistoryMessage[];
  total: number;
  start: number;
}

/**
 * A workspace-snapshot ref the session recorded (`Session.checkpoints` —
 * Cline shadow-git `metadata.checkpoint` history). `createdAt` is epoch ms.
 */
export interface SessionCheckpoint {
  ref: string;
  createdAt: number;
  runCount?: number;
  kind?: string;
}

/** POST /api/sessions/:id/rewind — how many nodes the cut kept/dropped. */
export interface RewindResult {
  kept: number;
  removed: number;
}

/** POST /api/sessions/:id/restore — per-file outcome report. */
export interface RestoreResult {
  restored: Array<{
    path: string;
    action: "written" | "deleted" | "unchanged";
    bytes?: number;
  }>;
  skipped: Array<{ path: string; reason: string }>;
}

export interface AttachResult {
  attached: boolean;
  readOnly: boolean;
  /** The agent the session attached under (server ≥ provenance spans). */
  agentId?: string;
  /** The attached agent's capability advertisement (absent on older peers). */
  capabilities?: AgentCapabilities;
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
