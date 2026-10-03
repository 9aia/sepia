import { isAbsolute } from "node:path";
import { Effect, Either, Layer, Option } from "effect";
import type { AcpConnection, AcpSessionInfo } from "sepia-acp";
import { createTranslator, type Event, type Translator } from "sepia-agui";
import { SessionRepository } from "sepia-core";
import type { Session } from "sepia-core";
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

/** Maps a store backend to the agent that can resume it. */
const agentForBackend = (backendType: string): string =>
  backendType === "cline" ? "cline" : "devin";

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
      });

    const historyLimit = (historyOptions?: HistoryOptions): number =>
      historyOptions?.limit !== undefined && Number.isFinite(historyOptions.limit)
        ? Math.max(0, Math.floor(historyOptions.limit))
        : envNumber(process.env.SEPIA_HISTORY_LIMIT, DEFAULT_HISTORY_LIMIT);

    const getHistory = (
      id: string,
      historyOptions?: HistoryOptions,
    ): Effect.Effect<HistoryPage, ControlError> =>
      Effect.gen(function* () {
        const maybe = yield* repo
          .getById(id)
          .pipe(Effect.mapError(storageFail("Failed to read session")));
        if (Option.isNone(maybe)) {
          return yield* Effect.fail(controlError("not_found", `Unknown session: ${id}`, undefined));
        }
        const nodes = maybe.value.nodes;
        const total = nodes.length;
        const limit = historyLimit(historyOptions);
        const slice = limit >= total ? nodes : nodes.slice(total - limit);
        return {
          messages: slice.map((node): HistoryMessage => ({
            role: node.role,
            content: node.content,
            createdAt: node.createdAt * 1000,
            toolName: Option.getOrUndefined(node.toolName),
          })),
          total,
        };
      });

    const agentForSession = (backendType: string): AgentRuntime | undefined =>
      options.agents.find((agent) => agent.id === agentForBackend(backendType)) ??
      pickAgent(options.agents, options.defaultAgentId);

    const performAttach = (id: string, takeover: boolean): Promise<AttachResult> =>
      Effect.runPromise(
        Effect.gen(function* () {
          const maybe = yield* repo
            .getById(id)
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
            agent.spawn({ cwd: session.workingDirectory }),
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
            return { attached: false, readOnly: true };
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
            if (locks.get(id)?.locked === true) return { attached: false, readOnly: true };
            return yield* Effect.fail(loaded.left);
          }
          emit(live, translator.endTurn());
          touchIdle(live);
          liveSessions.set(id, live);
          return { attached: true, readOnly: false };
        }),
      );

    const attach = (
      id: string,
      attachOptions?: { readonly takeover?: boolean },
    ): Effect.Effect<AttachResult, ControlError> =>
      Effect.gen(function* () {
        if (liveSessions.has(id)) return { attached: true, readOnly: false };

        // Claim the id before the first await so concurrent attaches share one spawn.
        let pending = pendingAttaches.get(id);
        if (pending === undefined) {
          pending = performAttach(id, attachOptions?.takeover === true);
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
      });

    const createSession = (createOptions: {
      readonly cwd: string;
      readonly agentId?: string;
      readonly title?: string;
    }): Effect.Effect<{ readonly id: string }, ControlError> =>
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

        const conn = yield* tryAcp("Failed to spawn agent", () => agent.spawn({ cwd }));
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
        return { id };
      });

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
      });

    const requireLive = (id: string): Effect.Effect<LiveSession, ControlError> =>
      Effect.gen(function* () {
        const live = liveSessions.get(id);
        if (live === undefined) {
          return yield* Effect.fail(
            controlError("invalid", `Session is not attached: ${id}`, undefined),
          );
        }
        return live;
      });

    const prompt = (id: string, text: string): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id);
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
      });

    const cancel = (id: string): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id);
        yield* tryAcp("Failed to cancel prompt", () => live.conn.cancel(id));
      });

    const respondToPermission = (
      id: string,
      requestId: string,
      optionId: string | null,
    ): Effect.Effect<void, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id);
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
      });

    const subscribe = (
      id: string,
      listener: SessionEventListener,
    ): Effect.Effect<Unsubscribe, ControlError> =>
      Effect.gen(function* () {
        const live = yield* requireLive(id);
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
        void Effect.runPromise(detach(id)).catch((error: unknown) =>
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
      createSession,
      attach,
      detach,
      prompt,
      cancel,
      respondToPermission,
      subscribe,
      listAgents,
      closeAll,
    };
  });

export const layer = (
  options: ControlPlaneOptions,
): Layer.Layer<ControlPlane, never, SessionRepository> => Layer.effect(ControlPlane, make(options));
