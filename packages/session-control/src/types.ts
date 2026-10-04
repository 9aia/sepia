/**
 * Public contract for the sepia control plane. Frozen seam between
 * `sepia-session-control` and `sepia-server`. Implementation lives in
 * `ControlPlane.ts`; keep this shape stable.
 */

import { Context, Schema, type Effect } from "effect";
import type { AcpConnection, PromptPart } from "sepia-acp";
import type { Event } from "sepia-agui";
import type {
  Block,
  Session,
  TokenUsage,
  ToolCallDiff,
  ToolCallLocation,
  ToolCallStatus,
} from "sepia-core";

/**
 * One run span of a session: which agent on which Sepia node continued it.
 * The session is a container — the same session can resume under different
 * agents/machines, so the meta overlay appends a span at each attach.
 */
export interface RunSpan {
  /** Epoch milliseconds when the span was recorded (attach time). */
  readonly at: number;
  readonly agent: string;
  readonly node: string;
}

export interface SessionSummary {
  readonly id: string;
  readonly title: string;
  readonly cwd: string;
  readonly agent: string;
  readonly updatedAt: string;
  readonly locked: boolean;
  readonly lockHolderPid: number | null;
  readonly source: string;
  readonly busy: boolean;
  /** Sepia-overlay metadata (not part of the agent's own store). */
  readonly pinned?: boolean;
  readonly archived?: boolean;
  readonly projectIds?: ReadonlyArray<string>;
  /** Run provenance from the meta overlay; empty until the first attach. */
  readonly spans?: ReadonlyArray<RunSpan>;
  /** Id of the session that spawned this one, when the store records a sub-agent tree. */
  readonly parentSessionId?: string;
  /** Sub-agent identity within the parent's team (Cline `agent_id`); not the agent runtime. */
  readonly agentId?: string;
}

export interface HistoryMessage {
  readonly role: "user" | "assistant" | "tool" | "system";
  readonly content: string;
  /**
   * The message's content blocks — present only when the store recorded
   * non-text content (images, file attachments). `content` is the joined
   * text projection and stays canonical.
   */
  readonly blocks?: ReadonlyArray<Block>;
  readonly createdAt: number;
  readonly toolName?: string;
  /** Reasoning text the store recorded (`"[redacted]"` marks an opaque block). */
  readonly thinking?: string;
  /** Opaque provider seal on `thinking` — replayed verbatim, never decoded. */
  readonly thinkingSignature?: string;
  /** Token metrics the agent's store recorded for this message. */
  readonly usage?: TokenUsage;
  readonly model?: string;
  readonly requestId?: string;
  readonly finishReason?: string;
  /** Tool-result messages only: how the call this message answers ended. */
  readonly toolStatus?: ToolCallStatus;
  readonly exitCode?: number;
  readonly durationMs?: number;
  /**
   * Tool-result messages only: the call's raw input args, JSON-encoded (a
   * single value — the same shape the live `args` stream accumulates).
   * Joined from the assistant node's `toolCalls` like `locations`/`diffs`.
   */
  readonly args?: string;
  /**
   * Tool-result messages only: files the call touched (`locations`, ACP)
   * and the before/after payloads the store recorded (`diffs`). Joined from
   * the assistant node's `toolCalls` by `toolCallId`.
   */
  readonly locations?: ReadonlyArray<ToolCallLocation>;
  readonly diffs?: ReadonlyArray<ToolCallDiff>;
  /**
   * Tool-result messages only: the call this message answers — the key a
   * per-call file restore (`restore` with `toolCallId`) reverts against.
   */
  readonly toolCallId?: string;
}

export interface AttachResult {
  readonly attached: boolean;
  readonly readOnly: boolean;
  /** The agent runtime the session is (or would be) attached under. */
  readonly agentId: string;
}

/**
 * A file-level restore against a session's recorded state — two sources,
 * picked by which of `path`/`checkpoint` is set:
 *
 * - `path` (+ optional `toolCallId`): revert the file through the recorded
 *   `ToolCall.diffs`. Without `toolCallId` the file goes back to its state
 *   before the session first touched it; with it, exactly that call's change
 *   is reverted. Reverts are non-clobbering — a file that drifted from the
 *   recorded after-state skips rather than being overwritten.
 * - `checkpoint`: a `Session.checkpoints` ref — either a Cline shadow-git
 *   stash/commit sha in the workspace repository (restores `ref^..ref`) or
 *   a Claude `file-history-snapshot` ref (materializes the recorded
 *   path→backup map from the agent's `file-history/` dir; `null` backups
 *   are deletion tombstones). `paths` narrows to a subset. A ref whose kind
 *   the store can't serve fails `conflict` rather than faking a restore.
 *
 * `confirm: true` is mandatory — every variant writes (or deletes) real
 * files under the session's working directory.
 */
export interface RestoreRequest {
  readonly confirm: boolean;
  readonly path?: string;
  readonly toolCallId?: string;
  readonly checkpoint?: string;
  readonly paths?: ReadonlyArray<string>;
}

export interface RestoredFile {
  /** Absolute path the restore touched. */
  readonly path: string;
  readonly action: "written" | "deleted" | "unchanged";
  readonly bytes?: number;
}

export interface SkippedFile {
  readonly path: string;
  readonly reason: string;
}

export interface RestoreResult {
  readonly restored: ReadonlyArray<RestoredFile>;
  readonly skipped: ReadonlyArray<SkippedFile>;
}

/**
 * The filesystem/git seam `restore` works through — injectable so tests (and
 * embedders) can fake the disk. `readFile` returns `null` for absent files;
 * `writeFile` creates parent directories.
 */
export interface RestoreExec {
  readonly readFile: (path: string) => Promise<Uint8Array | null>;
  readonly writeFile: (path: string, content: Uint8Array) => Promise<void>;
  readonly removeFile: (path: string) => Promise<void>;
  readonly git: (
    cwd: string,
    args: ReadonlyArray<string>,
  ) => Promise<{ readonly code: number; readonly stdout: Uint8Array; readonly stderr: string }>;
}

export interface HistoryPage {
  readonly messages: ReadonlyArray<HistoryMessage>;
  readonly total: number;
  /** Absolute index of `messages[0]` within the full backlog; `> 0` means more
   * history exists earlier — pass it as `before` to fetch the previous page. */
  readonly start: number;
}

export interface HistoryOptions {
  /** Number of trailing messages to return; defaults to `SEPIA_HISTORY_LIMIT`. */
  readonly limit?: number;
  /** Exclusive end index for the slice; defaults to the backlog end. */
  readonly before?: number;
  /** Resolve the id within this agent's store — ids collide across agents. */
  readonly agentId?: string;
}

export interface AgentInfo {
  readonly id: string;
  readonly label: string;
}

/** One runnable ACP agent binary, injected so tests can fake it. */
export interface AgentRuntime extends AgentInfo {
  readonly spawn: (options: {
    readonly cwd: string;
    readonly model?: string;
    readonly fallbacks?: ReadonlyArray<string>;
  }) => Promise<AcpConnection>;
}

export type SessionEventListener = (events: ReadonlyArray<Event>) => void;
export type Unsubscribe = () => void;

export const ControlErrorCode = Schema.Literal(
  "not_found",
  "invalid",
  "locked",
  "unknown_agent",
  "conflict",
  "busy",
  "internal",
);

export type ControlErrorCode = Schema.Schema.Type<typeof ControlErrorCode>;

export class ControlError extends Schema.TaggedError<ControlError>()("ControlError", {
  code: ControlErrorCode,
  message: Schema.String,
  cause: Schema.Unknown,
}) {}

export interface ControlPlaneService {
  /** Sessions from the sepia IR store; `withLocks` also asks the default agent for live lock state. */
  readonly listSessions: (options?: {
    readonly withLocks?: boolean;
  }) => Effect.Effect<ReadonlyArray<SessionSummary>, ControlError>;

  readonly getHistory: (
    id: string,
    options?: HistoryOptions,
  ) => Effect.Effect<HistoryPage, ControlError>;

  /**
   * The complete session IR — nodes with toolCalls ids/args, thinking, usage
   * and parent links — served by `GET /api/sessions/:id/export` so a peer
   * node's `/import` can resume without the history projection's losses.
   */
  readonly getSession: (
    id: string,
    options?: { readonly agentId?: string },
  ) => Effect.Effect<Session, ControlError>;

  /** Spawns an agent, creates a fresh session, and registers it live so it can be prompted immediately. */
  readonly createSession: (options: {
    readonly cwd: string;
    readonly agentId?: string;
    readonly title?: string;
    readonly model?: string;
    readonly fallbacks?: ReadonlyArray<string>;
  }) => Effect.Effect<{ readonly id: string; readonly agentId: string }, ControlError>;

  /**
   * Spawns the session's agent and loads the session. Locked sessions attach
   * read-only unless `takeover`, which SIGTERMs the lock-holder pid the agent
   * reports before loading; a takeover that still cannot load fails `locked`
   * rather than silently degrading to read-only. `agentId` scopes the store
   * lookup — ids collide across agents.
   */
  readonly attach: (
    id: string,
    options?: {
      readonly takeover?: boolean;
      readonly model?: string;
      readonly fallbacks?: ReadonlyArray<string>;
      readonly agentId?: string;
    },
  ) => Effect.Effect<AttachResult, ControlError>;

  readonly detach: (id: string) => Effect.Effect<void, ControlError>;

  /**
   * Sends one turn: `parts` is the ACP `session/prompt` content-block list —
   * a `text` part plus any attachment blocks (`image`, `audio`, `resource`,
   * `resource_link`). Forwarded to the agent verbatim.
   */
  readonly prompt: (
    id: string,
    parts: ReadonlyArray<PromptPart>,
    agentId?: string,
  ) => Effect.Effect<void, ControlError>;

  readonly cancel: (id: string, agentId?: string) => Effect.Effect<void, ControlError>;

  /** Detaches if live, then deletes the session through its agent runtime. `agentId` scopes the store lookup. */
  readonly deleteSession: (
    id: string,
    options?: { readonly agentId?: string },
  ) => Effect.Effect<void, ControlError>;

  readonly respondToPermission: (
    id: string,
    requestId: string,
    optionId: string | null,
    agentId?: string,
  ) => Effect.Effect<void, ControlError>;

  /**
   * Restores files under the session's working directory — either
   * reverse-applying recorded `ToolCall.diffs` for a `path` or materializing
   * a `Session.checkpoints` shadow-git ref. Refused while the session is
   * busy or locked by a live process; requires `confirm: true`. `agentId`
   * scopes the store lookup — ids collide across agents.
   */
  readonly restore: (
    id: string,
    request: RestoreRequest,
    agentId?: string,
  ) => Effect.Effect<RestoreResult, ControlError>;

  readonly subscribe: (
    id: string,
    listener: SessionEventListener,
    agentId?: string,
  ) => Effect.Effect<Unsubscribe, ControlError>;

  readonly listAgents: () => ReadonlyArray<AgentInfo>;

  readonly closeAll: () => Effect.Effect<void>;
}

export interface ControlPlaneOptions {
  readonly agents: ReadonlyArray<AgentRuntime>;
  readonly defaultAgentId?: string;
  /** Directory the lock probe runs in; defaults to `process.cwd()`. */
  readonly probeCwd?: string;
  /** Detach a live session after this long with no listeners and no turn; 0 disables. Defaults to `SEPIA_IDLE_TTL_MS`. */
  readonly idleTtlMs?: number;
  /** How often the idle sweep runs. Defaults to `SEPIA_SWEEP_MS`. */
  readonly sweepMs?: number;
  /**
   * Signals the lock-holder pid during an explicit takeover — SIGTERM via
   * `process.kill` by default. Injectable so tests can fake the process table;
   * only ever invoked with the pid the agent itself reported.
   */
  readonly terminateLockHolder?: (pid: number) => void;
  /**
   * The disk/git seam for `restore` — defaults to real `node:fs` + `git`
   * subprocess calls. Inject in tests; a restore never touches the agent's
   * session store, only files under the session's working directory.
   */
  readonly restoreExec?: RestoreExec;
  /**
   * Directory Claude's `file-history-snapshot` backups live under —
   * `<dir>/<sessionId>/<backupName>` — read by checkpoint restores whose ref
   * `kind` is `file-history-snapshot`. Defaults to
   * `~/.claude/file-history`; the server passes `$SEPIA_CLAUDE_DIR/file-history`.
   */
  readonly fileHistoryDir?: string;
}

export class ControlPlane extends Context.Tag("ControlPlane")<
  ControlPlane,
  ControlPlaneService
>() {}
