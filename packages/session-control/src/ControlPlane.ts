import { isAbsolute } from "node:path";
import { Effect, Either, Layer, Metric, Option, Runtime } from "effect";
import type { AcpConnection, AcpSessionInfo } from "sepia-acp";
import { createTranslator, type Event, type Translator } from "sepia-agui";
import { SessionRepository } from "sepia-core";
import type { Session } from "sepia-core";
import { agentForBackend } from "./MergedRepository.js";
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
  type SessionEventListener,
  type SessionSummary,
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

const DEFAULT_HISTORY_LIMIT = 500;
const DEFAULT_LOCK_TTL_MS = 5_000;
const DEFAULT_IDLE_TTL_MS = 600_000;
const DEFAULT_SWEEP_MS = 30_000;

const controlError = (code: ControlErrorCode, message: string, cause: unknown): ControlError =>
  new ControlError({ code, message, cause });

const tryAcp = <A>(message: string, thunk: () => Promise<A>): Effect.Effect<A, ControlError> =>
  Effect.tryPromise({ try: thunk, catch: (cause) => controlError("internal", message, cause) });

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
    const pendingAttaches = new Map<string, Promise<AttachResult>>();
    const probeCwd = options.probeCwd ?? process.cwd();
    const idleTtlMs =
      options.idleTtlMs ?? envNumber(process.env.SEPIA_IDLE_TTL_MS, DEFAULT_IDLE_TTL_MS);
    const sweepMs = options.sweepMs ?? envNumber(process.env.SEPIA_SWEEP_MS, DEFAULT_SWEEP_MS);
    let lockCache: {
      readonly at: number;
      readonly locks: ReadonlyMap<string, AcpSessionInfo>;
    } | null = null;

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

    // Reuse a live connection when one exists; otherwise spawn a throwaway agent
    // and close it. The result is cached briefly so listing does not spawn per call.
    const probeLocks = (cwd: string): Effect.Effect<ReadonlyArray<AcpSessionInfo>, never> => {
      const live = [...liveSessions.values()][0];
      if (live !== undefined) {
        return tryAcp("Failed to list agent sessions", () => live.conn.listSessions()).pipe(
          Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)),
        );
      }
      const agent = pickAgent(options.agents, options.defaultAgentId);
      if (agent === undefined) return Effect.succeed([]);
      return Effect.gen(function* () {
        const conn = yield* tryAcp("Failed to spawn agent for lock check", () =>
          agent.spawn({ cwd }),
        );
        return yield* tryAcp("Failed to list agent sessions", () => conn.listSessions()).pipe(
          Effect.ensuring(
            tryAcp("Failed to close agent connection", () => conn.close()).pipe(Effect.ignore),
          ),
          Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)),
        );
      }).pipe(Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)));
    };

    const lockState = (
      cwd: string,
      force = false,
    ): Effect.Effect<ReadonlyMap<string, AcpSessionInfo>, never> =>
      Effect.gen(function* () {
        const now = Date.now();
        const ttl = envNumber(process.env.SEPIA_LOCK_TTL_MS, DEFAULT_LOCK_TTL_MS);
        if (!force && lockCache !== null && now - lockCache.at < ttl) return lockCache.locks;
        const infos = yield* probeLocks(cwd);
        const locks = new Map(infos.map((info) => [info.sessionId, info]));
        lockCache = { at: Date.now(), locks };
        return locks;
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
        const locks = yield* lockState(probeCwd);
        return all.map((summary) => {
          const lock = locks.get(summary.id);
          return lock === undefined
            ? summary
            : { ...summary, locked: lock.locked, lockHolderPid: lock.lockHolderPid };
        });
      }).pipe(Effect.withSpan("sepia.control.list_sessions"));

    const historyLimit = (historyOptions?: HistoryOptions): number =>
      historyOptions?.limit !== undefined && Number.isFinite(historyOptions.limit)
        ? Math.max(1, Math.floor(historyOptions.limit))
        : envNumber(process.env.SEPIA_HISTORY_LIMIT, DEFAULT_HISTORY_LIMIT);

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
        return {
          messages: slice.map((node): HistoryMessage => {
            const toolResult = Option.getOrUndefined(node.toolResult);
            return {
              role: node.role,
              content: node.content,
              blocks: node.blocks.length === 0 ? undefined : node.blocks,
              createdAt: node.createdAt * 1000,
              toolName: Option.getOrUndefined(node.toolName),
              usage: Option.getOrUndefined(node.usage),
              model: Option.getOrUndefined(node.model),
              requestId: Option.getOrUndefined(node.requestId),
              finishReason: Option.getOrUndefined(node.finishReason),
              toolStatus: toolResult?.status,
              exitCode: toolResult?.exitCode,
              durationMs: toolResult?.durationMs,
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

    const agentForSession = (backendType: string): AgentRuntime | undefined =>
      options.agents.find((agent) => agent.id === agentForBackend(backendType)) ??
      pickAgent(options.agents, options.defaultAgentId);

    // Mutable cell refreshed inside method gens; sweepIdle uses whatever the
    // last request fiber saw (or the make-time ambient if none ran yet).
    let attachRuntime: Runtime.Runtime<never> = yield* Effect.runtime<never>();

    const performAttach = (
      id: string,
      takeover: boolean,
      attachOptions?: {
        readonly model?: string;
        readonly fallbacks?: ReadonlyArray<string>;
        readonly agentId?: string;
      },
    ): Promise<AttachResult> =>
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

          const conn = yield* tryAcp("Failed to spawn agent", () =>
            agent.spawn({
              cwd: session.workingDirectory,
              model: attachOptions?.model,
              fallbacks: attachOptions?.fallbacks,
            }),
          );
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
          const infos = yield* tryAcp("Failed to list agent sessions", () =>
            conn.listSessions(),
          ).pipe(Effect.catchAll(() => Effect.succeed([] as ReadonlyArray<AcpSessionInfo>)));
          const info = infos.find((candidate) => candidate.sessionId === id);
          if (info?.locked === true && !takeover) {
            yield* teardown;
            yield* close;
            return { attached: false, readOnly: true, agentId: agent.id };
          }

          emit(live, translator.startRun());
          const loaded = yield* Effect.either(
            tryAcp("Failed to load session", () => conn.loadSession(id, session.workingDirectory)),
          );
          if (Either.isLeft(loaded)) {
            yield* teardown;
            yield* close;
            emit(live, translator.endTurn());
            // A load failure is authoritative: re-probe once, treating a lock as read-only.
            const locks = yield* lockState(session.workingDirectory, true);
            if (locks.get(id)?.locked === true) {
              return { attached: false, readOnly: true, agentId: agent.id };
            }
            return yield* Effect.fail(loaded.left);
          }
          emit(live, translator.endTurn());
          touchIdle(live);
          liveSessions.set(id, live);
          return { attached: true, readOnly: false, agentId: agent.id };
        }).pipe(
          Effect.withSpan("sepia.control.attach_work", {
            attributes: { "sepia.session.id": id },
          }),
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
            return { attached: true, readOnly: false, agentId: existing.agentId };
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
        return yield* Effect.tryPromise({
          try: () => pending,
          catch: (cause) => cause as ControlError,
        });
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
    }): Effect.Effect<{ readonly id: string; readonly agentId: string }, ControlError> =>
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

        const conn = yield* tryAcp("Failed to spawn agent", () =>
          agent.spawn({
            cwd,
            model: createOptions.model,
            fallbacks: createOptions.fallbacks,
          }),
        );
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
        return { id, agentId: agent.id };
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
      text: string,
      agentId?: string,
    ): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id, agentId);
        if (live.busy) {
          return yield* Effect.fail(controlError("busy", `Session is busy: ${id}`, undefined));
        }
        live.busy = true;
        live.idleSince = null;
        emit(live, live.translator.startRun());
        yield* tryAcp("Failed to send prompt", () =>
          live.conn.prompt(id, [{ type: "text", text }]),
        ).pipe(
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
          pickAgent(options.agents, options.defaultAgentId);
        if (agent === undefined) {
          return yield* Effect.fail(
            controlError("unknown_agent", `No agent available for session: ${id}`, undefined),
          );
        }

        const conn = yield* tryAcp("Failed to spawn agent", () => agent.spawn({ cwd }));
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
      options.agents.map(({ id, label }) => ({ id, label }));

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
      subscribe,
      listAgents,
      closeAll,
    };
  });

export const layer = (
  options: ControlPlaneOptions,
): Layer.Layer<ControlPlane, never, SessionRepository> => Layer.effect(ControlPlane, make(options));
