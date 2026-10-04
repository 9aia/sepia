/**
 * Public contract for the sepia control plane. Frozen seam between
 * `sepia-session-control` and `sepia-server`. Implementation lives in
 * `ControlPlane.ts`; keep this shape stable.
 */

import { Context, Schema, type Effect } from "effect";
import type { AcpConnection } from "sepia-acp";
import type { Event } from "sepia-agui";
import type { Session, TokenUsage, ToolCallStatus } from "sepia-core";

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
  readonly createdAt: number;
  readonly toolName?: string;
  /** Token metrics the agent's store recorded for this message. */
  readonly usage?: TokenUsage;
  readonly model?: string;
  readonly requestId?: string;
  readonly finishReason?: string;
  /** Tool-result messages only: how the call this message answers ended. */
  readonly toolStatus?: ToolCallStatus;
  readonly exitCode?: number;
  readonly durationMs?: number;
}

export interface AttachResult {
  readonly attached: boolean;
  readonly readOnly: boolean;
  /** The agent runtime the session is (or would be) attached under. */
  readonly agentId: string;
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
   * read-only unless `takeover`. `agentId` scopes the store lookup — ids
   * collide across agents.
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

  readonly prompt: (
    id: string,
    text: string,
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
}

export class ControlPlane extends Context.Tag("ControlPlane")<
  ControlPlane,
  ControlPlaneService
>() {}
