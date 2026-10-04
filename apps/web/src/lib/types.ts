export type AgentKind = "devin" | "cline";

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

export interface HistoryMessage {
  role: HistoryRole;
  content: string;
  createdAt: number;
  toolName?: string;
}

export interface HistoryPage {
  messages: HistoryMessage[];
  total: number;
  start: number;
}

export interface AttachResult {
  attached: boolean;
  readOnly: boolean;
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
