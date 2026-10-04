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

/**
 * `agentCapabilities.promptCapabilities` from the initialize response —
 * which `session/prompt` content blocks beyond the baseline (`text` +
 * `resource_link`, which every agent must take) the agent accepts. ACP
 * defaults every flag to false when unadvertised, so these normalize to
 * required booleans.
 */
export interface AcpPromptCapabilities {
  readonly image: boolean;
  readonly audio: boolean;
  /** Whether `resource` content blocks (embedded context) are accepted. */
  readonly embeddedContext: boolean;
}

/**
 * `agentCapabilities.sessionCapabilities` flattened to booleans — each ACP
 * entry is an object (possibly empty) whose presence advertises the method.
 */
export interface AcpSessionCapabilities {
  readonly list: boolean;
  readonly delete: boolean;
  readonly fork: boolean;
  readonly resume: boolean;
  readonly close: boolean;
  readonly additionalDirectories: boolean;
}

/** The agent's capability advertisement, captured at `initialize`. */
export interface AcpCapabilities {
  readonly loadSession: boolean;
  /** `sessionCapabilities.list` flattened — kept for existing callers. */
  readonly sessionList: boolean;
  readonly promptCapabilities: AcpPromptCapabilities;
  readonly sessionCapabilities: AcpSessionCapabilities;
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
  readonly line?: number;
}

/**
 * A file change reported in a tool call's `content` — the ACP
 * `{type:"diff", path, oldText?, newText?}` entries. Absent `oldText` marks
 * a create, absent `newText` a delete.
 */
export interface ToolCallDiff {
  readonly path: string;
  readonly oldText?: string;
  readonly newText?: string;
}

/**
 * A non-diff `content` entry of a tool call — the ACP
 * `{type:"terminal", terminalId}` refs (with `output` when the agent inlines
 * the terminal text) and wrapped `ContentBlock`s (`{type:"content", content}`
 * — text and image blocks are kept, other block kinds dropped).
 */
export type ToolCallContent =
  | { readonly type: "terminal"; readonly terminalId: string; readonly output?: string }
  | { readonly type: "text"; readonly text: string }
  | {
      readonly type: "image";
      readonly data?: string;
      readonly uri?: string;
      readonly mimeType?: string;
    };

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
      readonly diffs: ReadonlyArray<ToolCallDiff>;
      /** Non-diff `content` entries — present only when the call carries some. */
      readonly contents?: ReadonlyArray<ToolCallContent>;
    }
  | {
      readonly kind: "tool_call_update";
      readonly toolCallId: string;
      readonly status: string;
      readonly title?: string;
      readonly rawInput?: unknown;
      readonly rawOutput?: unknown;
      readonly locations?: ReadonlyArray<ToolCallLocation>;
      readonly diffs?: ReadonlyArray<ToolCallDiff>;
      readonly contents?: ReadonlyArray<ToolCallContent>;
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

/**
 * A content block of an ACP `session/prompt` request — the subset of the
 * schema's `ContentBlock` union Sepia sends. `resource` embeds a payload
 * (text or base64 `blob`), `resource_link` references one by URI; `data`
 * on image/audio is base64.
 */
export type PromptPart =
  | { readonly type: "text"; readonly text: string }
  | {
      readonly type: "image";
      readonly data: string;
      readonly mimeType: string;
      readonly uri?: string;
    }
  | { readonly type: "audio"; readonly data: string; readonly mimeType: string }
  | {
      readonly type: "resource";
      readonly resource:
        | { readonly uri: string; readonly mimeType?: string; readonly text: string }
        | { readonly uri: string; readonly mimeType?: string; readonly blob: string };
    }
  | {
      readonly type: "resource_link";
      readonly uri: string;
      readonly name: string;
      readonly mimeType?: string;
      readonly size?: number;
    };

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
