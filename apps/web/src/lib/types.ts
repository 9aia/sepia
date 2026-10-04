export type AgentKind = "devin" | "cline";

export interface SessionSummary {
  id: string;
  title: string;
  cwd: string;
  agent: AgentKind;
  updatedAt: string;
  locked: boolean;
  lockHolderPid: number | null;
  source: string;
  busy: boolean;
  pinned: boolean;
  projectId: string | null;
  model: string | null;
}

export interface Project {
  id: string;
  name: string;
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
