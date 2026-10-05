import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { Effect, Either, Layer, Metric, Option, Runtime } from "effect";
import type {
  AcpCapabilities,
  AcpConnection,
  AcpPromptCapabilities,
  AcpSessionInfo,
  PromptPart,
} from "sepia-acp";
import { createTranslator, type Event, type Translator } from "sepia-agui";
import { Restore, Rewind, SessionRepository } from "sepia-core";
import type { Session, ToolCall } from "sepia-core";
import { agentForBackend } from "./MergedRepository.js";
import { defaultRestoreExec } from "./restore-exec.js";
import {
  ControlError,
  ControlPlane,
  type AgentInfo,
  type AgentRuntime,
  type AttachResult,
  type ControlErrorCode,
  type ControlPlaneOptions,
  type ControlPlaneService,
  type HistoryMessage,
  type HistoryOptions,
  type HistoryPage,
  type RestoreExec,
  type RestoreRequest,
  type RestoreResult,
  type RestoredFile,
  type RewindRequest,
  type RewindResult,
  type SessionEventListener,
  type SessionSummary,
  type SkippedFile,
  type Unsubscribe,
} from "./types.js";

// Exported over OTLP when the server merges the telemetry layer; no-ops otherwise.
const metricAttaches = Metric.counter("sepia_attach_total");
const metricCreates = Metric.counter("sepia_sessions_created_total");
const metricDeletes = Metric.counter("sepia_sessions_deleted_total");
const metricPrompts = Metric.counter("sepia_prompts_total");

interface LiveSession {
  readonly conn: AcpConnection;
  readonly translator: Translator;
  readonly listeners: Set<SessionEventListener>;
  readonly unsubs: Unsubscribe[];
  readonly cwd: string;
  readonly title: string;
  readonly agentId: string;
  busy: boolean;
  /** Set while the session has no listeners and no in-flight turn. */
  idleSince: number | null;
}

/**
 * One `session/list` fan-out across every registered agent. `merged` is the
 * union keyed by session id — per-agent views can disagree on a colliding
 * id, so `locked: true` in ANY agent's view counts as held. `byAgent` keeps
 * each agent's own view so a caller can prefer the report from the agent
 * that owns the session's backend (its lock-holder pid is the one a
 * takeover must signal).
 */
interface LockProbe {
  readonly merged: ReadonlyMap<string, AcpSessionInfo>;
  readonly byAgent: ReadonlyMap<string, ReadonlyMap<string, AcpSessionInfo>>;
}

/**
 * The owning agent's view of a session, falling back to the merged union
 * when that agent did not list the id at all. A colliding id locked in a
 * different agent's view still counts as held — it just can't supply the
 * holder pid once the owning agent lists the session itself.
 */
const probeInfoFor = (
  probe: LockProbe,
  agentId: string,
  sessionId: string,
): AcpSessionInfo | undefined =>
  probe.byAgent.get(agentId)?.get(sessionId) ?? probe.merged.get(sessionId);

const DEFAULT_HISTORY_LIMIT = 500;
const DEFAULT_LOCK_TTL_MS = 5_000;
const DEFAULT_IDLE_TTL_MS = 600_000;
const DEFAULT_SWEEP_MS = 30_000;
// A signaled holder needs a moment to flush and drop the session lock —
// poll session/list for this long before giving the load a shot anyway.
const TAKEOVER_SETTLE_MS = 800;
const TAKEOVER_POLL_MS = 100;
// A store lock can lag the holder's exit — an explicit takeover retries the
// load once after this delay.
const TAKEOVER_RETRY_DELAY_MS = 300;

const controlError = (code: ControlErrorCode, message: string, cause: unknown): ControlError =>
  new ControlError({ code, message, cause });

const tryAcp = <A>(message: string, thunk: () => Promise<A>): Effect.Effect<A, ControlError> =>
  Effect.tryPromise({ try: thunk, catch: (cause) => controlError("internal", message, cause) });

/**
 * The `promptCapabilities` flag a non-baseline content block needs —
 * `text` and `resource_link` are baseline (ACP requires every agent to
 * take them), so they have no entry. `resource` blocks are embedded
 * context. Unadvertised flags normalize to false on the connection, so a
 * miss here fails the prompt before the agent errors opaquely mid-turn.
 */
const PROMPT_PART_CAPABILITY: Readonly<
  Partial<
    Record<
      PromptPart["type"],
      { readonly flag: keyof AcpPromptCapabilities; readonly label: string }
    >
  >
> = {
  image: { flag: "image", label: "image" },
  audio: { flag: "audio", label: "audio" },
  resource: { flag: "embeddedContext", label: "embedded context (resource)" },
};

/** The first part the agent can't take, or null when every part is allowed. */
const disallowedPart = (
  conn: AcpConnection,
  parts: ReadonlyArray<PromptPart>,
): { readonly type: PromptPart["type"]; readonly label: string } | null => {
  for (const part of parts) {
    const gate = PROMPT_PART_CAPABILITY[part.type];
    if (gate !== undefined && conn.capabilities.promptCapabilities[gate.flag] !== true) {
      return { type: part.type, label: gate.label };
    }
  }
  return null;
};

const emit = (live: LiveSession, events: ReadonlyArray<Event>): void => {
  for (const listener of live.listeners) {
    try {
      listener(events);
    } catch (error) {
      console.error(`sepia-session-control: listener threw for session ${live.title}:`, error);
    }
  }
};

const touchIdle = (live: LiveSession): void => {
  live.idleSince = live.busy || live.listeners.size > 0 ? null : Date.now();
};

const envNumber = (raw: string | undefined, fallback: number): number => {
  if (raw === undefined || raw === "") return fallback;
  const value = Number(raw);
  return Number.isFinite(value) && value >= 0 ? Math.floor(value) : fallback;
};

const toSummary = (session: Session): SessionSummary => {
  const agent = agentForBackend(session.backendType);
  return {
    id: session.id,
    title: session.title,
    cwd: session.workingDirectory,
    agent,
    updatedAt: new Date(session.lastActivityAt * 1000).toISOString(),
    locked: false,
    lockHolderPid: null,
    source: agent,
    busy: false,
    parentSessionId: Option.getOrUndefined(session.parentSessionId),
    agentId: Option.getOrUndefined(session.agentId),
  };
};

const pickAgent = (
  agents: ReadonlyArray<AgentRuntime>,
  defaultAgentId: string | undefined,
): AgentRuntime | undefined =>
  defaultAgentId === undefined ? agents[0] : agents.find((agent) => agent.id === defaultAgentId);

export const make = (
  options: ControlPlaneOptions,
): Effect.Effect<ControlPlaneService, never, SessionRepository> =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const liveSessions = new Map<string, LiveSession>();
    const pendingAttaches = new Map<string, Promise<Either.Either<AttachResult, ControlError>>>();
    const probeCwd = options.probeCwd ?? process.cwd();
    const idleTtlMs =
      options.idleTtlMs ?? envNumber(process.env.SEPIA_IDLE_TTL_MS, DEFAULT_IDLE_TTL_MS);
    const sweepMs = options.sweepMs ?? envNumber(process.env.SEPIA_SWEEP_MS, DEFAULT_SWEEP_MS);
    let lockCache: {
      readonly at: number;
      readonly probe: LockProbe;
    } | null = null;

    // The last `initialize` advertisement seen per agent, refreshed on every
    // spawn — `listAgents` reports it so callers (and UIs) can see what a
    // peer's agent takes before attaching.
    const probedCapabilities = new Map<string, AcpCapabilities>();

    /** `agent.spawn` plus the capability probe record. */
    const spawn = (
      agent: AgentRuntime,
      message: string,
      spawnOptions: Parameters<AgentRuntime["spawn"]>[0],
    ): Effect.Effect<AcpConnection, ControlError> =>
      tryAcp(message, () => agent.spawn(spawnOptions)).pipe(
        Effect.tap((conn) =>
          Effect.sync(() => {
            probedCapabilities.set(agent.id, conn.capabilities);
          }),
        ),
      );

    const storageFail = (message: string) => (cause: unknown) =>
      controlError("internal", message, cause);

    // Live sessions are keyed by bare id; when the caller scopes to an agent,
    // a live entry for a different agent's colliding id must not match.
    const liveFor = (id: string, agentId?: string): LiveSession | undefined => {
      const live = liveSessions.get(id);
      return live !== undefined && (agentId === undefined || live.agentId === agentId)
        ? live
        : undefined;
    };

    /**
     * Can this agent answer a lock probe at all? `sessionList` is the RPC
     * the probe calls; `loadSession` marks an agent that can hold the kind
     * of lock a takeover would need to release — one that cannot load can
     * never be that holder. `undefined` means no spawn has happened yet, so
     * the capabilities are unknown — probe once and let the spawn learn them.
     */
    const probeable = (capabilities: AcpCapabilities | undefined): boolean =>
      capabilities === undefined ||
      (capabilities.sessionList === true && capabilities.loadSession === true);

    /**
     * One `session/list` per registered agent — a lock held under a backend
     * the default agent can't see (e.g. a cline session held by a running
     * cline process during a devin probe) still surfaces. A live attach's
     * connection is reused for its own agent; `borrowed` covers a connection
     * about to go live, so attach's lock check doubles as that agent's probe.
     * Every other agent gets a throwaway spawn that is closed after listing.
     * An agent already probed as incapable is skipped — no spawn, no RPC.
     * The probes run in parallel — each is a subprocess spawn — and a
     * failing probe contributes an empty view.
     */
    const probeLocks = (
      cwd: string,
      borrowed?: { readonly agentId: string; readonly conn: AcpConnection },
    ): Effect.Effect<LockProbe, never> => {
      const listOn = (conn: AcpConnection): Effect.Effect<ReadonlyArray<AcpSessionInfo>, never> =>
        tryAcp("Failed to list agent sessions", () => conn.listSessions()).pipe(
          Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)),
        );
      const empty = (
        agentId: string,
      ): { readonly agentId: string; readonly infos: ReadonlyArray<AcpSessionInfo> } => ({
        agentId,
        infos: [],
      });
      const probeOne = (
        agent: AgentRuntime,
      ): Effect.Effect<
        { readonly agentId: string; readonly infos: ReadonlyArray<AcpSessionInfo> },
        never
      > => {
        const reusable =
          borrowed?.agentId === agent.id
            ? borrowed.conn
            : [...liveSessions.values()].find((live) => live.agentId === agent.id)?.conn;
        if (reusable !== undefined) {
          if (!probeable(reusable.capabilities)) return Effect.succeed(empty(agent.id));
          return Effect.map(listOn(reusable), (infos) => ({ agentId: agent.id, infos }));
        }
        if (!probeable(probedCapabilities.get(agent.id))) {
          return Effect.succeed(empty(agent.id));
        }
        return Effect.gen(function* () {
          const conn = yield* spawn(agent, "Failed to spawn agent for lock check", { cwd });
          // The throwaway spawn doubles as the capability probe — an agent
          // that turns out incapable has nothing to list this round either.
          const list = probeable(conn.capabilities)
            ? listOn(conn)
            : Effect.succeed([] as ReadonlyArray<AcpSessionInfo>);
          return yield* list.pipe(
            Effect.ensuring(
              tryAcp("Failed to close agent connection", () => conn.close()).pipe(Effect.ignore),
            ),
          );
        }).pipe(
          Effect.map((infos) => ({ agentId: agent.id, infos })),
          Effect.catchAll(() => Effect.succeed(empty(agent.id))),
        );
      };
      return Effect.all(options.agents.map(probeOne), { concurrency: "unbounded" }).pipe(
        Effect.map((probes) => {
          const byAgent = new Map<string, Map<string, AcpSessionInfo>>();
          const merged = new Map<string, AcpSessionInfo>();
          for (const { agentId, infos } of probes) {
            const view = new Map<string, AcpSessionInfo>();
            byAgent.set(agentId, view);
            for (const info of infos) {
              view.set(info.sessionId, info);
              const current = merged.get(info.sessionId);
              // A session locked in any agent's view counts as held.
              if (current === undefined || (info.locked === true && current.locked !== true)) {
                merged.set(info.sessionId, info);
              }
            }
          }
          return { merged, byAgent };
        }),
      );
    };

    const lockState = (
      cwd: string,
      force = false,
      borrowed?: { readonly agentId: string; readonly conn: AcpConnection },
    ): Effect.Effect<LockProbe, never> =>
      Effect.gen(function* () {
        const now = Date.now();
        const ttl = envNumber(process.env.SEPIA_LOCK_TTL_MS, DEFAULT_LOCK_TTL_MS);
        if (!force && lockCache !== null && now - lockCache.at < ttl) return lockCache.probe;
        const probe = yield* probeLocks(cwd, borrowed);
        lockCache = { at: Date.now(), probe };
        return probe;
      });

    const liveSummary = (id: string, live: LiveSession): SessionSummary => ({
      id,
      title: live.title,
      cwd: live.cwd,
      agent: live.agentId,
      updatedAt: new Date().toISOString(),
      locked: false,
      lockHolderPid: null,
      source: "sepia",
      busy: live.busy,
    });

    const listSessions = (listOptions?: {
      readonly withLocks?: boolean;
    }): Effect.Effect<ReadonlyArray<SessionSummary>, ControlError> =>
      Effect.gen(function* () {
        const sessions = yield* repo
          .list()
          .pipe(Effect.mapError(storageFail("Failed to list sessions")));
        const summaries = sessions.map(toSummary);
        const stored = new Set(summaries.map((summary) => summary.id));
        const synthesized = [...liveSessions.entries()]
          .filter(([id]) => !stored.has(id))
          .map(([id, live]) => liveSummary(id, live));
        const all = [...summaries, ...synthesized];
        if (listOptions?.withLocks !== true) return all;
        const probe = yield* lockState(probeCwd);
        return all.map((summary) => {
          const lock = probe.merged.get(summary.id);
          if (lock === undefined) return summary;
          // `locked` is the union — held in any agent's view counts — but
          // the holder pid prefers the report from the agent owning the
          // session's backend: a pid from a colliding id in another agent's
          // list names a holder that is not holding this session.
          const holder = probeInfoFor(probe, summary.agent, summary.id);
          return {
            ...summary,
            locked: lock.locked,
            lockHolderPid: lock.locked === true ? (holder?.lockHolderPid ?? null) : null,
          };
        });
      }).pipe(Effect.withSpan("sepia.control.list_sessions"));

    const historyLimit = (historyOptions?: HistoryOptions): number =>
      historyOptions?.limit !== undefined && Number.isFinite(historyOptions.limit)
        ? Math.max(1, Math.floor(historyOptions.limit))
        : envNumber(process.env.SEPIA_HISTORY_LIMIT, DEFAULT_HISTORY_LIMIT);

    /**
     * `ToolCall.arguments` is `unknown` in the IR — stores keep the parsed
     * arg object, a few keep the raw JSON string. Re-encode to one JSON
     * value so the flat row matches the live `args` stream's shape.
     */
    const callArgsText = (value: unknown): string | undefined => {
      if (value === undefined || value === null) return undefined;
      if (typeof value === "string") return value === "" ? undefined : value;
      // `{}` is what adapters record for arg-less calls — leave the field off.
      if (
        typeof value === "object" &&
        !Array.isArray(value) &&
        Object.keys(value as Record<string, unknown>).length === 0
      ) {
        return undefined;
      }
      try {
        return JSON.stringify(value);
      } catch {
        return undefined;
      }
    };

    const getHistory = (
      id: string,
      historyOptions?: HistoryOptions,
    ): Effect.Effect<HistoryPage, ControlError> =>
      Effect.gen(function* () {
        const maybe = yield* repo
          .getById(id, historyOptions?.agentId)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        if (Option.isNone(maybe)) {
          // A live (attached) session may not exist in the store yet — the
          // agent only flushes it after the first prompt. Treat as empty.
          if (liveFor(id, historyOptions?.agentId) !== undefined)
            return { messages: [], total: 0, start: 0 };
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        const nodes = maybe.value.nodes;
        const total = nodes.length;
        const limit = historyLimit(historyOptions);
        const before =
          historyOptions?.before !== undefined && Number.isFinite(historyOptions.before)
            ? Math.min(Math.max(0, Math.floor(historyOptions.before)), total)
            : total;
        const start = Math.max(0, before - limit);
        const slice = nodes.slice(start, before);
        // Tool rows render the call's file footprint, which lives on the
        // assistant node's `toolCalls` — join by toolCallId. The map covers
        // the whole backlog so a paged-in slice still resolves its calls.
        const callsById = new Map<string, ToolCall>();
        for (const node of nodes) {
          for (const call of node.toolCalls) callsById.set(call.id, call);
        }
        return {
          messages: slice.map((node): HistoryMessage => {
            const toolResult = Option.getOrUndefined(node.toolResult);
            const call =
              node.role === "tool"
                ? callsById.get(Option.getOrUndefined(node.toolCallId) ?? "")
                : undefined;
            return {
              role: node.role,
              nodeId: node.nodeId,
              content: node.content,
              blocks: node.blocks.length === 0 ? undefined : node.blocks,
              createdAt: node.createdAt * 1000,
              toolName: Option.getOrUndefined(node.toolName),
              thinking: Option.getOrUndefined(node.thinking),
              thinkingSignature: Option.getOrUndefined(node.thinkingSignature),
              usage: Option.getOrUndefined(node.usage),
              model: Option.getOrUndefined(node.model),
              requestId: Option.getOrUndefined(node.requestId),
              finishReason: Option.getOrUndefined(node.finishReason),
              toolStatus: toolResult?.status,
              exitCode: toolResult?.exitCode,
              durationMs: toolResult?.durationMs,
              args: call === undefined ? undefined : callArgsText(call.arguments),
              locations:
                call === undefined || call.locations.length === 0 ? undefined : call.locations,
              diffs: call === undefined || call.diffs.length === 0 ? undefined : call.diffs,
              toolCallId: node.role === "tool" ? Option.getOrUndefined(node.toolCallId) : undefined,
            };
          }),
          total,
          start,
        };
      }).pipe(
        Effect.withSpan("sepia.control.get_history", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const getSession = (
      id: string,
      getOptions?: { readonly agentId?: string },
    ): Effect.Effect<Session, ControlError> =>
      Effect.gen(function* () {
        const maybe = yield* repo
          .getById(id, getOptions?.agentId)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        if (Option.isNone(maybe)) {
          // A live, unflushed session has no durable IR yet; not_found sends
          // resume clients down the history path, which pages it as empty.
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        return maybe.value;
      }).pipe(
        Effect.withSpan("sepia.control.get_session", {
          attributes: { "sepia.session.id": id },
        }),
      );

    // The default-agent fallback only applies to backends that resolve to
    // the primary agent anyway — a backend mapped to a concrete agent id
    // that has no registered runtime (e.g. "cursor", a store with no live agent)
    // must fail unknown_agent rather than attach under the wrong agent.
    const agentForSession = (backendType: string): AgentRuntime | undefined => {
      const mapped = agentForBackend(backendType);
      const found = options.agents.find((agent) => agent.id === mapped);
      if (found !== undefined) return found;
      return mapped === "devin" ? pickAgent(options.agents, options.defaultAgentId) : undefined;
    };

    // Mutable cell refreshed inside method gens; sweepIdle uses whatever the
    // last request fiber saw (or the make-time ambient if none ran yet).
    let attachRuntime: Runtime.Runtime<never> = yield* Effect.runtime<never>();

    // Takeover signals the pid the agent reported as the lock holder —
    // SIGTERM (graceful; the holder is usually a devin TUI on this machine),
    // never SIGKILL, never an unreported or guessed pid.
    const terminateLockHolder =
      options.terminateLockHolder ??
      ((pid: number): void => {
        process.kill(pid, "SIGTERM");
      });

    /**
     * Releases the lock an explicit takeover needs: SIGTERM the reported
     * holder pid, then poll `session/list` until the lock clears or the
     * settle window ends. A missing/invalid pid, an already-dead holder
     * (ESRCH), or a refused signal all just fall through — the subsequent
     * load attempt is the arbiter.
     */
    const releaseLockHolder = (
      conn: AcpConnection,
      sessionId: string,
      pid: number | null,
    ): Effect.Effect<void> =>
      Effect.gen(function* () {
        if (pid === null || !Number.isInteger(pid) || pid <= 0 || pid === process.pid) return;
        const signaled = yield* Effect.sync(() => {
          try {
            terminateLockHolder(pid);
            return true;
          } catch {
            // ESRCH — the holder already exited; nothing left to release.
            return false;
          }
        });
        if (!signaled) return;
        // No `session/list` means the release can never be observed — let
        // the load attempt arbitrate instead of polling a missing RPC.
        if (conn.capabilities.sessionList !== true) return;
        const deadline = Date.now() + TAKEOVER_SETTLE_MS;
        while (Date.now() < deadline) {
          yield* Effect.sleep(TAKEOVER_POLL_MS);
          const current = yield* tryAcp("Failed to list agent sessions", () =>
            conn.listSessions(),
          ).pipe(Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)));
          if (current.find((candidate) => candidate.sessionId === sessionId)?.locked !== true) {
            return;
          }
        }
      });

    const performAttach = (
      id: string,
      takeover: boolean,
      attachOptions?: {
        readonly model?: string;
        readonly fallbacks?: ReadonlyArray<string>;
        readonly agentId?: string;
      },
    ): Promise<Either.Either<AttachResult, ControlError>> =>
      Runtime.runPromise(attachRuntime)(
        Effect.gen(function* () {
          const maybe = yield* repo
            .getById(id, attachOptions?.agentId)
            .pipe(Effect.mapError(storageFail("Failed to read session")));
          if (Option.isNone(maybe)) {
            return yield* Effect.fail(
              controlError("not_found", `Unknown session: ${id}`, undefined),
            );
          }
          const session = maybe.value;

          const agent = agentForSession(session.backendType);
          if (agent === undefined) {
            return yield* Effect.fail(
              controlError(
                "unknown_agent",
                `No agent available for session: ${id}`,
                options.defaultAgentId,
              ),
            );
          }

          const conn = yield* spawn(agent, "Failed to spawn agent", {
            cwd: session.workingDirectory,
            model: attachOptions?.model,
            fallbacks: attachOptions?.fallbacks,
          });
          const translator = createTranslator({ threadId: id });
          const live: LiveSession = {
            conn,
            translator,
            listeners: new Set(),
            unsubs: [],
            cwd: session.workingDirectory,
            title: session.title,
            agentId: agent.id,
            busy: false,
            idleSince: null,
          };
          live.unsubs.push(conn.onUpdate((update) => emit(live, translator.translate(update))));
          live.unsubs.push(
            conn.onPermission((request) => emit(live, translator.permissionRequest(request))),
          );

          const close = tryAcp("Failed to close agent connection", () => conn.close()).pipe(
            Effect.ignore,
          );
          const teardown = Effect.sync(() => {
            for (const unsub of live.unsubs) unsub();
          });
          // The lock check fans out to every registered agent — a session a
          // different backend's runtime holds is still seen — with this
          // attach's fresh connection standing in for its own agent's probe,
          // so the holder pid is the one the owning agent reported.
          const probe = yield* lockState(session.workingDirectory, true, {
            agentId: agent.id,
            conn,
          });
          const info = probeInfoFor(probe, agent.id, id);
          if (info?.locked === true) {
            if (!takeover) {
              yield* teardown;
              yield* close;
              return {
                attached: false,
                readOnly: true,
                agentId: agent.id,
                capabilities: conn.capabilities,
              };
            }
            // ACP session/load has no steal flag, so the only way to take a
            // held session is for the holder to let go — signal the pid the
            // agent itself reported, then let the load decide.
            yield* releaseLockHolder(conn, id, info.lockHolderPid);
          }

          emit(live, translator.startRun());
          const load = tryAcp("Failed to load session", () =>
            conn.loadSession(id, session.workingDirectory),
          );
          let loaded = yield* Effect.either(load);
          if (Either.isLeft(loaded) && takeover) {
            yield* Effect.sleep(TAKEOVER_RETRY_DELAY_MS);
            loaded = yield* Effect.either(load);
          }
          if (Either.isLeft(loaded)) {
            yield* teardown;
            yield* close;
            emit(live, translator.endTurn());
            // A load failure is authoritative: re-probe once, treating a lock as read-only.
            const reprobe = yield* lockState(session.workingDirectory, true);
            const held = probeInfoFor(reprobe, agent.id, id);
            if (held?.locked === true) {
              // A takeover that still sees the lock held failed — report it
              // instead of quietly degrading to read-only, which callers
              // cannot tell apart from "never tried". Name the holder pid
              // when the agent reported one so the UI can say what wouldn't
              // let go.
              if (takeover) {
                const heldPid = held.lockHolderPid ?? info?.lockHolderPid ?? null;
                return yield* Effect.fail(
                  controlError(
                    "locked",
                    heldPid !== null
                      ? `Session is held by PID ${heldPid} — it couldn't be released: ${id}`
                      : `Session is held by another process — the lock couldn't be released: ${id}`,
                    loaded.left,
                  ),
                );
              }
              return {
                attached: false,
                readOnly: true,
                agentId: agent.id,
                capabilities: conn.capabilities,
              };
            }
            return yield* Effect.fail(loaded.left);
          }
          emit(live, translator.endTurn());
          touchIdle(live);
          liveSessions.set(id, live);
          return {
            attached: true,
            readOnly: false,
            agentId: agent.id,
            capabilities: conn.capabilities,
          };
        }).pipe(
          Effect.withSpan("sepia.control.attach_work", {
            attributes: { "sepia.session.id": id },
          }),
          Effect.either,
        ),
      );

    const attach = (
      id: string,
      attachOptions?: {
        readonly takeover?: boolean;
        readonly model?: string;
        readonly fallbacks?: ReadonlyArray<string>;
        readonly agentId?: string;
      },
    ): Effect.Effect<AttachResult, ControlError> =>
      Effect.gen(function* () {
        const existing = liveSessions.get(id);
        if (existing !== undefined) {
          if (attachOptions?.agentId === undefined || existing.agentId === attachOptions.agentId) {
            return {
              attached: true,
              readOnly: false,
              agentId: existing.agentId,
              capabilities: existing.conn.capabilities,
            };
          }
          // The id is held by another agent's live session; liveSessions is
          // keyed by bare id and cannot host both copies at once.
          return yield* Effect.fail(
            controlError(
              "conflict",
              `Session is already attached under a different agent: ${id}`,
              undefined,
            ),
          );
        }

        // Captured here (not at make-time): the request fiber's refs carry the
        // OTLP tracer and the parent span, so performAttach's nested runPromise
        // exports and parents correctly. A make-time capture sees neither.
        attachRuntime = yield* Effect.runtime<never>();

        // Claim the id before the first await so concurrent attaches share one spawn.
        let pending = pendingAttaches.get(id);
        if (pending === undefined) {
          pending = performAttach(id, attachOptions?.takeover === true, attachOptions);
          pendingAttaches.set(id, pending);
          const settled = pending;
          void settled
            .catch(() => undefined)
            .finally(() => {
              if (pendingAttaches.get(id) === settled) pendingAttaches.delete(id);
            });
        }
        const outcome = yield* Effect.tryPromise({
          try: () => pending,
          catch: (cause) => cause as ControlError,
        });
        // performAttach resolves an Either so the ControlError — code and
        // all — survives the nested-runPromise hop that a rejection would
        // flatten into a FiberFailure.
        return yield* outcome;
      }).pipe(
        Effect.tap((result) => (result.attached ? Metric.increment(metricAttaches) : Effect.void)),
        Effect.withSpan("sepia.control.attach", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const createSession = (createOptions: {
      readonly cwd: string;
      readonly agentId?: string;
      readonly title?: string;
      readonly model?: string;
      readonly fallbacks?: ReadonlyArray<string>;
    }): Effect.Effect<
      {
        readonly id: string;
        readonly agentId: string;
        readonly capabilities: AcpCapabilities;
      },
      ControlError
    > =>
      Effect.gen(function* () {
        const cwd = createOptions.cwd;
        if (cwd.trim() === "" || !isAbsolute(cwd)) {
          return yield* Effect.fail(
            controlError("invalid", `cwd must be a non-empty absolute path: ${cwd}`, undefined),
          );
        }

        const agent =
          createOptions.agentId === undefined
            ? pickAgent(options.agents, options.defaultAgentId)
            : options.agents.find((candidate) => candidate.id === createOptions.agentId);
        if (agent === undefined) {
          return yield* Effect.fail(
            controlError(
              "unknown_agent",
              `Unknown agent: ${createOptions.agentId ?? options.defaultAgentId ?? ""}`,
              undefined,
            ),
          );
        }

        const conn = yield* spawn(agent, "Failed to spawn agent", {
          cwd,
          model: createOptions.model,
          fallbacks: createOptions.fallbacks,
        });
        const close = tryAcp("Failed to close agent connection", () => conn.close()).pipe(
          Effect.ignore,
        );
        const id = yield* tryAcp("Failed to create session", () => conn.newSession(cwd)).pipe(
          Effect.tapError(() => close),
        );
        const translator = createTranslator({ threadId: id });
        const live: LiveSession = {
          conn,
          translator,
          listeners: new Set(),
          unsubs: [],
          cwd,
          title: createOptions.title ?? "New session",
          agentId: agent.id,
          busy: false,
          idleSince: null,
        };
        live.unsubs.push(conn.onUpdate((update) => emit(live, translator.translate(update))));
        live.unsubs.push(
          conn.onPermission((request) => emit(live, translator.permissionRequest(request))),
        );
        touchIdle(live);
        liveSessions.set(id, live);
        return { id, agentId: agent.id, capabilities: conn.capabilities };
      }).pipe(
        Effect.tap(() => Metric.increment(metricCreates)),
        Effect.withSpan("sepia.control.create_session", {
          attributes: {
            "sepia.agent.id": createOptions.agentId ?? options.defaultAgentId ?? "",
            "sepia.cwd": createOptions.cwd,
          },
        }),
      );

    const detach = (id: string): Effect.Effect<void> =>
      Effect.gen(function* () {
        const live = liveSessions.get(id);
        if (live === undefined) return;
        liveSessions.delete(id);
        for (const unsub of live.unsubs) unsub();
        live.listeners.clear();
        yield* tryAcp("Failed to close agent connection", () => live.conn.close()).pipe(
          Effect.ignore,
        );
      }).pipe(
        Effect.withSpan("sepia.control.detach", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const requireLive = (id: string, agentId?: string): Effect.Effect<LiveSession, ControlError> =>
      Effect.gen(function* () {
        const live = liveFor(id, agentId);
        if (live === undefined) {
          return yield* Effect.fail(
            controlError("invalid", `Session is not attached: ${id}`, undefined),
          );
        }
        return live;
      });

    const prompt = (
      id: string,
      parts: ReadonlyArray<PromptPart>,
      agentId?: string,
    ): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id, agentId);
        // Fail fast on content the agent advertised it can't take — letting
        // it through surfaces as an opaque turn error mid-run instead.
        const disallowed = disallowedPart(live.conn, parts);
        if (disallowed !== null) {
          return yield* Effect.fail(
            controlError(
              "invalid",
              `Agent ${live.agentId} does not accept ${disallowed.label} prompt content: ${id}`,
              undefined,
            ),
          );
        }
        if (live.busy) {
          return yield* Effect.fail(controlError("busy", `Session is busy: ${id}`, undefined));
        }
        live.busy = true;
        live.idleSince = null;
        emit(live, live.translator.startRun());
        yield* tryAcp("Failed to send prompt", () => live.conn.prompt(id, parts)).pipe(
          Effect.ensuring(
            Effect.sync(() => {
              live.busy = false;
              touchIdle(live);
              emit(live, live.translator.endTurn());
            }),
          ),
        );
      }).pipe(
        Effect.tap(() => Metric.increment(metricPrompts)),
        Effect.withSpan("sepia.control.prompt", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const cancel = (id: string, agentId?: string): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id, agentId);
        yield* tryAcp("Failed to cancel prompt", () => live.conn.cancel(id));
      }).pipe(
        Effect.withSpan("sepia.control.cancel", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const deleteSession = (
      id: string,
      deleteOptions?: { readonly agentId?: string },
    ): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        // A live handle keeps the session open inside the agent; drop ours first
        // so our own lock does not make the delete fail. A live entry for a
        // different agent's colliding id is left alone.
        const live = liveFor(id, deleteOptions?.agentId);
        if (live !== undefined) yield* detach(id);

        const maybe = yield* repo
          .getById(id, deleteOptions?.agentId)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        const cwd = live?.cwd ?? (Option.isSome(maybe) ? maybe.value.workingDirectory : undefined);
        const agentId =
          deleteOptions?.agentId ??
          live?.agentId ??
          (Option.isSome(maybe) ? agentForBackend(maybe.value.backendType) : undefined);
        if (cwd === undefined) {
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        const agent =
          options.agents.find((candidate) => candidate.id === agentId) ??
          (agentId === undefined || agentId === "devin"
            ? pickAgent(options.agents, options.defaultAgentId)
            : undefined);
        if (agent === undefined) {
          return yield* Effect.fail(
            controlError("unknown_agent", `No agent available for session: ${id}`, undefined),
          );
        }

        const conn = yield* spawn(agent, "Failed to spawn agent", { cwd });
        yield* tryAcp("Failed to delete session", () => conn.deleteSession(id)).pipe(
          Effect.ensuring(
            tryAcp("Failed to close agent connection", () => conn.close()).pipe(Effect.ignore),
          ),
        );
      }).pipe(
        Effect.tap(() => Metric.increment(metricDeletes)),
        Effect.withSpan("sepia.control.delete_session", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const respondToPermission = (
      id: string,
      requestId: string,
      optionId: string | null,
      agentId?: string,
    ): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id, agentId);
        const settled = yield* Effect.try({
          try: () => live.conn.respondToPermission(requestId, optionId),
          catch: (cause) =>
            controlError("internal", "Failed to respond to permission request", cause),
        });
        if (!settled) {
          return yield* Effect.fail(
            controlError("not_found", `Unknown permission request: ${requestId}`, undefined),
          );
        }
      }).pipe(
        Effect.withSpan("sepia.control.respond_to_permission", {
          attributes: { "sepia.session.id": id },
        }),
      );

    /* ---- restore -------------------------------------------------------
     * File restore only — the IR has no deletion model, so a "rewind" of the
     * conversation itself isn't representable. Three sources, all gated on
     * `confirm: true` and refused while the session is busy or held by a
     * live process:
     *  - path restore: reverse-apply the recorded `ToolCall.diffs`
     *    (`Restore.planPathRestore` — skips rather than clobbering drift);
     *  - checkpoint restore, shadow-git: materialize the files a
     *    `Session.checkpoints` ref covers (`git show <ref>:<path>`);
     *  - checkpoint restore, file-history (`kind` =
     *    `Restore.FILE_HISTORY_KIND`): copy the blobs the snapshot's
     *    path→backup map names out of `<fileHistoryDir>/<sessionId>/`;
     *    `null` backups are deletion tombstones.
     */

    // Claude's `file-history/<sessionId>/` root; the server passes
    // `$SEPIA_CLAUDE_DIR/file-history` so SEPIA_CLAUDE_DIR relocates it.
    const fileHistoryDir = options.fileHistoryDir ?? `${homedir()}/.claude/file-history`;

    const exec: RestoreExec = options.restoreExec ?? defaultRestoreExec;
    const utf8 = new TextDecoder();
    const utf8Encode = new TextEncoder();

    const tryExec = <A>(message: string, thunk: () => Promise<A>) =>
      Effect.tryPromise({ try: thunk, catch: (cause) => controlError("internal", message, cause) });

    const git = (cwd: string, args: ReadonlyArray<string>, message: string) =>
      tryExec(message, () => exec.git(cwd, args));

    const bytesEqual = (a: Uint8Array, b: Uint8Array): boolean =>
      a.length === b.length && a.every((byte, index) => byte === b[index]);

    /** Revert `path` through the session's recorded diffs. */
    const restorePath = (
      session: Session,
      path: string,
      toolCallId?: string,
    ): Effect.Effect<RestoreResult, ControlError> =>
      Effect.gen(function* () {
        const abs = Restore.resolveWorkspacePath(session.workingDirectory, path);
        if (abs === null) {
          return yield* Effect.fail(
            controlError(
              "invalid",
              `path must resolve inside the session working directory: ${path}`,
              undefined,
            ),
          );
        }
        const raw = yield* tryExec(`Failed to read ${abs}`, () => exec.readFile(abs));
        const plan = Restore.planPathRestore(
          session,
          path,
          raw === null ? null : utf8.decode(raw),
          toolCallId,
        );
        switch (plan.kind) {
          case "skip":
            return { restored: [], skipped: [{ path, reason: plan.reason }] };
          case "unchanged":
            return { restored: [{ path: abs, action: "unchanged" }], skipped: [] };
          case "delete":
            yield* tryExec(`Failed to delete ${abs}`, () => exec.removeFile(abs));
            return { restored: [{ path: abs, action: "deleted" }], skipped: [] };
          case "write": {
            const bytes = utf8Encode.encode(plan.content);
            yield* tryExec(`Failed to write ${abs}`, () => exec.writeFile(abs, bytes));
            return {
              restored: [{ path: abs, action: "written", bytes: bytes.length }],
              skipped: [],
            };
          }
        }
      });

    /** Store-recorded ids/names become path segments — keep them single safe file names. */
    const isSafeFileName = (name: string): boolean =>
      name !== "" && name !== "." && name !== ".." && !/[/\\\0]/.test(name);

    /**
     * Materialize a Claude `file-history-snapshot` checkpoint: copy each
     * tracked file's backup blob out of `<fileHistoryDir>/<sessionId>/` into
     * the workspace path it covers. `backup: null` is the deletion
     * tombstone — the file did not exist at that checkpoint, so a present
     * file is removed. A ref with no recorded map fails `conflict`: the
     * store can't serve it and faking a restore would lie.
     */
    const restoreFileHistory = (
      session: Session,
      ref: string,
      paths?: ReadonlyArray<string>,
    ): Effect.Effect<RestoreResult, ControlError> =>
      Effect.gen(function* () {
        const snapshot = Restore.fileHistorySnapshot(session, ref);
        if (snapshot === undefined || !isSafeFileName(snapshot.sessionId)) {
          return yield* Effect.fail(
            controlError(
              "conflict",
              `Checkpoint ${ref} carries no file map — restore is not supported for this store`,
              undefined,
            ),
          );
        }
        const cwd = session.workingDirectory;
        const backupDir = join(fileHistoryDir, snapshot.sessionId);
        const skipped: SkippedFile[] = [];
        const covered: Array<{ tracked: string; abs: string; backup: string | null }> = [];
        for (const [tracked, info] of Object.entries(snapshot.files)) {
          const abs = Restore.resolveWorkspacePath(cwd, tracked);
          if (abs === null) {
            skipped.push({ path: tracked, reason: "outside the session working directory" });
            continue;
          }
          covered.push({ tracked, abs, backup: info.backup });
        }

        let targets = covered;
        if (paths !== undefined) {
          targets = [];
          for (const p of paths) {
            const abs = Restore.resolveWorkspacePath(cwd, p);
            if (abs === null) {
              return yield* Effect.fail(
                controlError(
                  "invalid",
                  `paths entries must resolve inside the session working directory: ${p}`,
                  undefined,
                ),
              );
            }
            const hit = covered.find((entry) => entry.abs === abs);
            if (hit === undefined) {
              skipped.push({ path: p, reason: `not touched by checkpoint ${ref}` });
            } else {
              targets.push(hit);
            }
          }
        }

        const restored: RestoredFile[] = [];
        for (const target of targets) {
          const existing = yield* tryExec(`Failed to read ${target.abs}`, () =>
            exec.readFile(target.abs),
          );
          if (target.backup === null) {
            if (existing === null) {
              restored.push({ path: target.abs, action: "unchanged" });
            } else {
              yield* tryExec(`Failed to delete ${target.abs}`, () => exec.removeFile(target.abs));
              restored.push({ path: target.abs, action: "deleted" });
            }
            continue;
          }
          if (!isSafeFileName(target.backup)) {
            skipped.push({ path: target.tracked, reason: "unsafe backup name recorded" });
            continue;
          }
          const backup = target.backup;
          const data = yield* tryExec("Failed to read file-history backup", () =>
            exec.readFile(join(backupDir, backup)),
          );
          if (data === null) {
            skipped.push({
              path: target.tracked,
              reason: `backup missing from the file-history store: ${target.backup}`,
            });
            continue;
          }
          if (existing !== null && bytesEqual(existing, data)) {
            restored.push({ path: target.abs, action: "unchanged" });
            continue;
          }
          yield* tryExec(`Failed to write ${target.abs}`, () => exec.writeFile(target.abs, data));
          restored.push({ path: target.abs, action: "written", bytes: data.length });
        }
        return { restored, skipped };
      });

    /**
     * Materialize the files a checkpoint covers. Shadow-git refs use the
     * `ref^..ref` name list (a stash ref's base is its first parent; a root
     * commit's is its whole tree); `file-history-snapshot` refs materialize
     * the recorded path→backup map instead. Files absent at the ref get
     * deleted — the checkpoint recorded them as removed.
     */
    const restoreCheckpoint = (
      session: Session,
      ref: string,
      paths?: ReadonlyArray<string>,
    ): Effect.Effect<RestoreResult, ControlError> =>
      Effect.gen(function* () {
        const entry = session.checkpoints.find((candidate) => candidate.ref === ref);
        if (entry === undefined) {
          return yield* Effect.fail(
            controlError("not_found", `Unknown checkpoint ref: ${ref}`, undefined),
          );
        }
        if (entry.kind === Restore.FILE_HISTORY_KIND) {
          return yield* restoreFileHistory(session, ref, paths);
        }
        const cwd = session.workingDirectory;
        const inside = yield* git(cwd, ["rev-parse", "--is-inside-work-tree"], "git probe failed");
        if (inside.code !== 0 || utf8.decode(inside.stdout).trim() !== "true") {
          return yield* Effect.fail(
            controlError(
              "conflict",
              `Session working directory is not a git work tree: ${cwd}`,
              undefined,
            ),
          );
        }
        const object = yield* git(cwd, ["cat-file", "-e", `${ref}^{commit}`], "git probe failed");
        if (object.code !== 0) {
          return yield* Effect.fail(
            controlError(
              "conflict",
              `Checkpoint ${ref} is not present in the workspace repository`,
              undefined,
            ),
          );
        }
        const rootResult = yield* git(cwd, ["rev-parse", "--show-toplevel"], "git probe failed");
        if (rootResult.code !== 0) {
          return yield* Effect.fail(
            controlError(
              "internal",
              `git rev-parse failed: ${rootResult.stderr.trim()}`,
              undefined,
            ),
          );
        }
        const root = utf8.decode(rootResult.stdout).trim();
        const parents = yield* git(
          cwd,
          ["rev-list", "--parents", "-n", "1", ref],
          "git probe failed",
        );
        const base = utf8.decode(parents.stdout).trim().split(/\s+/)[1];
        const coveredResult = yield* git(
          cwd,
          base === undefined
            ? ["ls-tree", "-r", "--name-only", "-z", ref]
            : ["diff", "--name-only", "-z", base, ref],
          `Failed to list files covered by checkpoint ${ref}`,
        );
        if (coveredResult.code !== 0) {
          return yield* Effect.fail(
            controlError(
              "internal",
              `git diff failed for checkpoint ${ref}: ${coveredResult.stderr.trim()}`,
              undefined,
            ),
          );
        }
        const covered = utf8
          .decode(coveredResult.stdout)
          .split("\0")
          .filter((entry) => entry !== "");

        // Git paths are repo-root relative; a restore only writes inside the
        // session's working directory.
        const toAbs = (rel: string): string => resolve(root, rel);
        const inCwd = (abs: string): boolean => Restore.resolveWorkspacePath(cwd, abs) !== null;

        const restored: RestoredFile[] = [];
        const skipped: SkippedFile[] = [];
        let targets: ReadonlyArray<string>;
        if (paths === undefined) {
          targets = covered;
        } else {
          const coveredSet = new Set(covered);
          const requested: string[] = [];
          for (const p of paths) {
            const abs = Restore.resolveWorkspacePath(cwd, p);
            if (abs === null) {
              return yield* Effect.fail(
                controlError(
                  "invalid",
                  `paths entries must resolve inside the session working directory: ${p}`,
                  undefined,
                ),
              );
            }
            const rel = relative(root, abs);
            if (!coveredSet.has(rel)) {
              skipped.push({ path: p, reason: `not touched by checkpoint ${ref}` });
            } else {
              requested.push(rel);
            }
          }
          targets = requested;
        }

        for (const rel of targets) {
          const abs = toAbs(rel);
          if (!inCwd(abs)) {
            skipped.push({ path: rel, reason: "outside the session working directory" });
            continue;
          }
          const present = yield* git(cwd, ["cat-file", "-e", `${ref}:${rel}`], "git probe failed");
          const existing = yield* tryExec(`Failed to read ${abs}`, () => exec.readFile(abs));
          if (present.code === 0) {
            const blob = yield* git(cwd, ["show", `${ref}:${rel}`], `Failed to read ${rel}`);
            if (blob.code !== 0) {
              skipped.push({
                path: rel,
                reason: `git show failed: ${blob.stderr.trim()}`,
              });
              continue;
            }
            if (existing !== null && bytesEqual(existing, blob.stdout)) {
              restored.push({ path: abs, action: "unchanged" });
              continue;
            }
            yield* tryExec(`Failed to write ${abs}`, () => exec.writeFile(abs, blob.stdout));
            restored.push({ path: abs, action: "written", bytes: blob.stdout.length });
          } else if (existing === null) {
            restored.push({ path: abs, action: "unchanged" });
          } else {
            yield* tryExec(`Failed to delete ${abs}`, () => exec.removeFile(abs));
            restored.push({ path: abs, action: "deleted" });
          }
        }
        return { restored, skipped };
      });

    const restore = (
      id: string,
      request: RestoreRequest,
      agentId?: string,
    ): Effect.Effect<RestoreResult, ControlError> =>
      Effect.gen(function* () {
        if (request.confirm !== true) {
          return yield* Effect.fail(
            controlError("invalid", "Restore writes files — pass confirm: true", undefined),
          );
        }
        const hasPath = request.path !== undefined;
        const hasCheckpoint = request.checkpoint !== undefined;
        if (hasPath === hasCheckpoint) {
          return yield* Effect.fail(
            controlError("invalid", "restore needs exactly one of path or checkpoint", undefined),
          );
        }
        const maybe = yield* repo
          .getById(id, agentId)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        if (Option.isNone(maybe)) {
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        const session = maybe.value;
        const live = liveFor(id, agentId);
        if (live !== undefined && live.busy) {
          return yield* Effect.fail(
            controlError("busy", `Session is busy — wait for the run to finish: ${id}`, undefined),
          );
        }
        // An unattached session can still be held by another process — the
        // same lock rule attach enforces. Our own live attach is the holder
        // we're checking through, so only probe when nothing is live here.
        if (live === undefined) {
          const probe = yield* lockState(session.workingDirectory);
          const lock = probeInfoFor(probe, agentForBackend(session.backendType), id);
          if (lock?.locked === true) {
            const pid = lock.lockHolderPid;
            return yield* Effect.fail(
              controlError(
                "locked",
                pid !== null
                  ? `Session is held by PID ${pid}: ${id}`
                  : `Session is held by another process: ${id}`,
                undefined,
              ),
            );
          }
        }
        return yield* hasCheckpoint
          ? restoreCheckpoint(session, request.checkpoint ?? "", request.paths)
          : restorePath(session, request.path ?? "", request.toolCallId);
      }).pipe(
        Effect.withSpan("sepia.control.restore", {
          attributes: { "sepia.session.id": id },
        }),
      );

    /* ---- rewind -------------------------------------------------------
     * Conversation truncation — the complement of restore's file writes.
     * The cut itself is pure IR math (`Rewind.planRewind`); persisting it
     * is delegated to the backend's injected `SessionRewinder`, since each
     * store truncates differently (row delete, array slice, JSONL rewrite,
     * checkpoint re-root). Same gates as restore: `confirm`, not busy, not
     * locked — plus a live idle attach is detached first, because the
     * agent's in-memory transcript would reflush the deleted tail.
     */
    const rewind = (
      id: string,
      request: RewindRequest,
      agentId?: string,
    ): Effect.Effect<RewindResult, ControlError> =>
      Effect.gen(function* () {
        if (request.confirm !== true) {
          return yield* Effect.fail(
            controlError(
              "invalid",
              "Rewind deletes stored history — pass confirm: true",
              undefined,
            ),
          );
        }
        const maybe = yield* repo
          .getById(id, agentId)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        if (Option.isNone(maybe)) {
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        const session = maybe.value;
        const live = liveFor(id, agentId);
        if (live !== undefined && live.busy) {
          return yield* Effect.fail(
            controlError("busy", `Session is busy — wait for the run to finish: ${id}`, undefined),
          );
        }
        if (live === undefined) {
          const probe = yield* lockState(session.workingDirectory);
          const lock = probeInfoFor(probe, agentForBackend(session.backendType), id);
          if (lock?.locked === true) {
            const pid = lock.lockHolderPid;
            return yield* Effect.fail(
              controlError(
                "locked",
                pid !== null
                  ? `Session is held by PID ${pid}: ${id}`
                  : `Session is held by another process: ${id}`,
                undefined,
              ),
            );
          }
        }

        const planned = Rewind.planRewind(session, {
          nodeId: request.nodeId,
          turns: request.turns,
          checkpoint: request.checkpoint,
        });
        if (!planned.ok) {
          return yield* Effect.fail(controlError("invalid", planned.reason, undefined));
        }
        const { plan } = planned;
        // Already at the requested point — nothing to write.
        if (plan.removed.length === 0) return { kept: plan.keepCount, removed: 0 };

        const rewinder = options.rewinders?.[agentForBackend(session.backendType)];
        if (rewinder === undefined) {
          return yield* Effect.fail(
            controlError(
              "conflict",
              `Rewind is not supported for this store: ${session.backendType}`,
              undefined,
            ),
          );
        }
        // A live agent holds the pre-rewind transcript in memory and flushes
        // it back on the next turn — drop the attach before writing.
        if (live !== undefined) yield* detach(id);
        yield* rewinder
          .truncate(session, plan, Rewind.rewindSession(session, plan))
          .pipe(
            Effect.mapError((cause) =>
              cause instanceof ControlError
                ? cause
                : controlError("internal", `Failed to truncate session: ${id}`, cause),
            ),
          );
        return { kept: plan.keepCount, removed: plan.removed.length };
      }).pipe(
        Effect.withSpan("sepia.control.rewind", {
          attributes: { "sepia.session.id": id },
        }),
      );

    const subscribe = (
      id: string,
      listener: SessionEventListener,
      agentId?: string,
    ): Effect.Effect<Unsubscribe, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id, agentId);
        live.listeners.add(listener);
        live.idleSince = null;
        return (): void => {
          live.listeners.delete(listener);
          touchIdle(live);
        };
      });

    const listAgents = (): ReadonlyArray<AgentInfo> =>
      options.agents.map(({ id, label }) => ({
        id,
        label,
        // `undefined` until the first spawn probes — JSON.stringify drops it,
        // so older wire consumers see the same shape as before.
        capabilities: probedCapabilities.get(id),
      }));

    // A live session nobody is listening to still owns an agent subprocess; reclaim
    // it once it has been idle (no listeners, no in-flight turn) for the TTL.
    const sweepIdle = (): void => {
      const now = Date.now();
      for (const [id, live] of liveSessions) {
        if (live.busy || live.listeners.size > 0 || live.idleSince === null) continue;
        if (now - live.idleSince < idleTtlMs) continue;
        void Runtime.runPromise(attachRuntime)(detach(id)).catch((error: unknown) =>
          console.error(`sepia-session-control: idle detach failed for ${id}:`, error),
        );
      }
    };

    let sweeper: ReturnType<typeof setInterval> | undefined;
    if (idleTtlMs > 0) {
      sweeper = setInterval(sweepIdle, sweepMs);
      (sweeper as { unref?: () => void }).unref?.();
    }

    const closeAll = (): Effect.Effect<void> =>
      Effect.gen(function* () {
        if (sweeper !== undefined) clearInterval(sweeper);
        yield* Effect.forEach([...liveSessions.keys()], (id) => detach(id), { discard: true });
      });

    return {
      listSessions,
      getHistory,
      getSession,
      createSession,
      attach,
      detach,
      prompt,
      cancel,
      deleteSession,
      respondToPermission,
      restore,
      rewind,
      subscribe,
      listAgents,
      closeAll,
    };
  });

export const layer = (
  options: ControlPlaneOptions,
): Layer.Layer<ControlPlane, never, SessionRepository> => Layer.effect(ControlPlane, make(options));
