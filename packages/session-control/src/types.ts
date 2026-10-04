/**
 * Public contract for the sepia control plane. Frozen seam between
 * `sepia-session-control` and `sepia-server`. Implementation lives in
 * `ControlPlane.ts`; keep this shape stable.
 */

import { Context, Schema, type Effect } from "effect";
import type { AcpConnection } from "sepia-acp";
import type { Event } from "sepia-agui";

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
  readonly projectIds?: ReadonlyArray<string>;
}

export interface HistoryMessage {
  readonly role: "user" | "assistant" | "tool" | "system";
  readonly content: string;
  readonly createdAt: number;
  readonly toolName?: string;
}

export interface AttachResult {
  readonly attached: boolean;
  readonly readOnly: boolean;
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

  /** Spawns an agent, creates a fresh session, and registers it live so it can be prompted immediately. */
  readonly createSession: (options: {
    readonly cwd: string;
    readonly agentId?: string;
    readonly title?: string;
    readonly model?: string;
    readonly fallbacks?: ReadonlyArray<string>;
  }) => Effect.Effect<{ readonly id: string }, ControlError>;

  /** Spawns the session's agent and loads the session. Locked sessions attach read-only unless `takeover`. */
  readonly attach: (
    id: string,
    options?: {
      readonly takeover?: boolean;
      readonly model?: string;
      readonly fallbacks?: ReadonlyArray<string>;
    },
  ) => Effect.Effect<AttachResult, ControlError>;

  readonly detach: (id: string) => Effect.Effect<void, ControlError>;

  readonly prompt: (id: string, text: string) => Effect.Effect<void, ControlError>;

  readonly cancel: (id: string) => Effect.Effect<void, ControlError>;

  /** Detaches if live, then deletes the session through its agent runtime. */
  readonly deleteSession: (id: string) => Effect.Effect<void, ControlError>;

  readonly respondToPermission: (
    id: string,
    requestId: string,
    optionId: string | null,
  ) => Effect.Effect<void, ControlError>;

  readonly subscribe: (
    id: string,
    listener: SessionEventListener,
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
