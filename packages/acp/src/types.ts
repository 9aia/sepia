/**
 * Public contract for the sepia ACP runtime. This file is the frozen seam
 * between `sepia-acp` and its consumers (`sepia-agui`, `sepia-session-control`).
 * Implementation lives in the sibling modules; keep this shape stable.
 */

export interface AgentSpec {
  readonly id: string;
  readonly label: string;
  readonly command: ReadonlyArray<string>;
  readonly env?: Readonly<Record<string, string>>;
}

export interface AcpCapabilities {
  readonly loadSession: boolean;
  readonly sessionList: boolean;
}

export interface AcpSessionInfo {
  readonly sessionId: string;
  readonly cwd: string;
  readonly title: string;
  readonly updatedAt: string;
  readonly locked: boolean;
  readonly lockHolderPid: number | null;
}

export interface ToolCallLocation {
  readonly path: string;
}

/** Normalized subset of ACP `session/update` notifications that we render. */
export type AcpSessionUpdate =
  | { readonly kind: "agent_message_chunk"; readonly text: string }
  | { readonly kind: "agent_thought_chunk"; readonly text: string }
  | { readonly kind: "user_message_chunk"; readonly text: string }
  | {
      readonly kind: "tool_call";
      readonly toolCallId: string;
      readonly title: string;
      readonly status: string;
      readonly toolKind: string;
      readonly rawInput: unknown;
      readonly locations: ReadonlyArray<ToolCallLocation>;
    }
  | {
      readonly kind: "tool_call_update";
      readonly toolCallId: string;
      readonly status: string;
      readonly title?: string;
      readonly rawOutput?: unknown;
    }
  | {
      readonly kind: "plan";
      readonly entries: ReadonlyArray<{ readonly content: string; readonly status: string }>;
    }
  | { readonly kind: "current_mode_update"; readonly modeId: string }
  | {
      readonly kind: "available_commands_update";
      readonly commands: ReadonlyArray<{ readonly name: string; readonly description?: string }>;
    }
  | { readonly kind: "other"; readonly sessionUpdate: string; readonly raw: unknown };

export interface PermissionOption {
  readonly optionId: string;
  readonly name: string;
  readonly kind: string;
}

export interface PermissionRequest {
  readonly requestId: string;
  readonly sessionId: string;
  readonly toolCallId: string | null;
  readonly title: string;
  readonly options: ReadonlyArray<PermissionOption>;
}

export type PromptPart = { readonly type: "text"; readonly text: string };

export type Unsubscribe = () => void;

export interface AcpConnection {
  readonly capabilities: AcpCapabilities;
  listSessions(): Promise<ReadonlyArray<AcpSessionInfo>>;
  newSession(cwd: string): Promise<string>;
  loadSession(sessionId: string, cwd: string): Promise<void>;
  prompt(sessionId: string, parts: ReadonlyArray<PromptPart>): Promise<void>;
  cancel(sessionId: string): Promise<void>;
  /** Deletes a session from the agent's store; rejects when the agent refuses. */
  deleteSession(sessionId: string): Promise<void>;
  /** Settles a pending permission request; returns false when the id is unknown. */
  respondToPermission(requestId: string, optionId: string | null): boolean;
  /** Bounded, prefixed tail of the agent's stderr for error reporting. */
  recentStderr(): ReadonlyArray<string>;
  onUpdate(listener: (update: AcpSessionUpdate) => void): Unsubscribe;
  onPermission(listener: (request: PermissionRequest) => void): Unsubscribe;
  close(): Promise<void>;
}

export interface SpawnOptions {
  readonly cwd: string;
  readonly env?: Readonly<Record<string, string>>;
  /** Preferred model — passed to the agent's spawn flag, applies at spawn. */
  readonly model?: string;
  /** Ordered fallback models (agent-specific flag, e.g. devin's refusal-fallback). */
  readonly fallbacks?: ReadonlyArray<string>;
}
