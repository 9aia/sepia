import { createHash, randomUUID, timingSafeEqual } from "node:crypto";
import { readdirSync } from "node:fs";
import { hostname, userInfo } from "node:os";
import { join } from "node:path";
import { Effect, Either } from "effect";
import { encodeSse, sseHeaders, type Event } from "sepia-agui";
import { Conversion, ClineStore, openSessionsDb, SqliteStorage } from "sepia-core";
import type { Session } from "sepia-core";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { Layer } from "effect";
import { ControlError } from "sepia-session-control";
import type {
  ControlErrorCode,
  ControlPlaneService,
  SessionEventListener,
  Unsubscribe,
} from "sepia-session-control";
import { createAguiAgentHandler } from "./agui-agent";
import type { MetaStore } from "./meta";
import { PROTOCOL_VERSION, type NodeIdentity } from "./node";
import type { ServerStore } from "./servers";
import { handleServersRoute } from "./servers-routes";
import type { TunnelManager } from "./ssh";
import { keepAliveMsFromEnv, SseChannel } from "./sse-channel";
import { makePushStore, notifyForEvents } from "./push";

export interface AppOptions {
  /** Runs effects; pass `runtime.runPromise` so spans/metrics reach the OTLP runtime. */
  readonly run?: EffectRunner;
  /** When set, every `/api/*` route except `GET /api/health` requires a bearer token. */
  readonly token?: string;
  /** Receives one line per request; defaults to `console.log`. */
  readonly logger?: (line: string) => void;
  /** Overrides the built-in CORS allowlist. */
  readonly allowedOrigins?: ReadonlyArray<string>;
  /** Overrides `SEPIA_SSE_KEEPALIVE_MS`; tests use a tiny value. */
  readonly keepAliveMs?: number;
  /** Session-title overlay; absent → `PATCH /api/sessions/:id` returns 501. */
  readonly meta?: MetaStore;
  /** Enables `POST /api/sessions/:id/convert` — needs the stores it converts between. */
  readonly convert?: { readonly dbPath: string; readonly clineDir: string };
  /**
   * Test seam for `POST /api/sessions/import`: replaces the store write
   * (which needs real bun:sqlite stores) while keeping request validation,
   * the IR rebuild and response shaping under test.
   */
  readonly importSession?: (
    session: Session,
    agent: "cline" | "devin",
  ) => Effect.Effect<string, unknown>;
  /**
   * Node identity reported by `GET /api/node` (docs/protocol.md). Absent → an
   * ephemeral id is minted for the process lifetime (tests, embedded use).
   */
  readonly node?: NodeIdentity;
  /** Managed-server registry; absent → /api/servers returns 501. */
  readonly servers?: ServerStore;
  /** SSH tunnel manager backing ssh-enabled registry entries. */
  readonly tunnels?: TunnelManager;
}

const ALLOWED_ORIGINS: ReadonlySet<string> = new Set([
  "http://localhost:3000",
  "http://127.0.0.1:3000",
]);

const HEALTH_TIMEOUT_MS = 1_500;

const CODE_STATUS: Readonly<Record<ControlErrorCode, number>> = {
  not_found: 404,
  invalid: 400,
  unknown_agent: 400,
  locked: 409,
  conflict: 409,
  busy: 409,
  internal: 500,
};

const corsHeaders = (
  origin: string | null,
  allowed: ReadonlySet<string>,
): Record<string, string> => {
  const headers: Record<string, string> = { vary: "Origin" };
  // "*" in SEPIA_ORIGINS echoes any Origin back — bearer auth (not CORS) is
  // the gate, and federation UIs on other machines must be able to call in.
  if (origin !== null && (allowed.has(origin) || allowed.has("*"))) {
    headers["access-control-allow-origin"] = origin;
    // PATCH/DELETE cover session meta, projects, config and deletes for
    // remote (federated) callers; same-origin calls don't consult this.
    headers["access-control-allow-methods"] = "GET,POST,PATCH,DELETE,OPTIONS";
    headers["access-control-allow-headers"] = "content-type,authorization";
  }
  return headers;
};

const jsonResponse = (body: unknown, status: number, cors: Record<string, string>): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...cors },
  });

// Hash both sides so the buffers are always equal length; a raw length mismatch
// would make timingSafeEqual throw and leak the expected token's length.
const tokenMatches = (provided: string, expected: string): boolean =>
  timingSafeEqual(
    createHash("sha256").update(provided).digest(),
    createHash("sha256").update(expected).digest(),
  );

const isAuthorized = (request: Request, token: string | undefined): boolean => {
  if (token === undefined || token === "") return true;
  const header = request.headers.get("authorization");
  // EventSource cannot set headers, so /stream clients authenticate via query.
  // The access log only records url.pathname, never query params.
  const provided = header?.startsWith("Bearer ")
    ? header.slice("Bearer ".length)
    : new URL(request.url).searchParams.get("access_token");
  return provided !== null && provided !== undefined && tokenMatches(provided, token);
};

const unauthorizedResponse = (cors: Record<string, string>): Response =>
  new Response(JSON.stringify({ error: "Unauthorized" }), {
    status: 401,
    headers: { "content-type": "application/json", "www-authenticate": "Bearer", ...cors },
  });

const errorMessage = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { readonly message: unknown }).message);
  }
  return String(error);
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const errorCode = (error: unknown): ControlErrorCode | undefined => {
  if (!isRecord(error)) return undefined;
  const code = error.code;
  return typeof code === "string" && code in CODE_STATUS ? (code as ControlErrorCode) : undefined;
};

const errorResponse = (error: unknown, cors: Record<string, string>): Response => {
  const message = errorMessage(error);
  const code = errorCode(error);
  if (code === undefined) return jsonResponse({ error: message }, 500, cors);
  return jsonResponse({ error: message, code }, CODE_STATUS[code], cors);
};

export type EffectRunner = <A, E>(effect: Effect.Effect<A, E>) => Promise<A>;

const respond = async <A>(
  run: EffectRunner,
  effect: Effect.Effect<A, ControlError>,
  cors: Record<string, string>,
  opts: {
    readonly shape?: (value: A) => unknown;
    readonly status?: number;
    readonly span?: string;
  } = {},
): Promise<Response> => {
  const shape = opts.shape ?? ((value: A) => value);
  const status = opts.status ?? 200;
  const spanned = opts.span === undefined ? effect : effect.pipe(Effect.withSpan(opts.span));
  // `Effect.either` keeps the raw `ControlError` (with its `code`) rather than the
  // `FiberFailure` wrapper that `runPromise` would reject with.
  const result = await run(Effect.either(spanned));
  if (Either.isLeft(result)) {
    void run(Effect.logWarning(`request failed: ${errorMessage(result.left)}`));
    return errorResponse(result.left, cors);
  }
  return jsonResponse(shape(result.right), status, cors);
};

const readJsonBody = async (request: Request): Promise<unknown> => {
  const text = await request.text();
  if (text.trim() === "") return undefined;
  return JSON.parse(text) as unknown;
};

const segmentsEqual = (segments: ReadonlyArray<string>, pattern: ReadonlyArray<string>): boolean =>
  segments.length === pattern.length && pattern.every((part, index) => part === segments[index]);

const healthResponse = async (
  run: EffectRunner,
  plane: ControlPlaneService,
  cors: Record<string, string>,
): Promise<Response> => {
  const healthy = await run(
    plane.listSessions().pipe(
      Effect.timeoutTo({
        duration: `${HEALTH_TIMEOUT_MS} millis`,
        onTimeout: () => false,
        onSuccess: () => true,
      }),
      Effect.catchAll(() => Effect.succeed(false)),
    ),
  );
  return healthy
    ? jsonResponse({ ok: true, db: true }, 200, cors)
    : jsonResponse({ ok: false, db: false }, 503, cors);
};

const streamResponse = async (
  run: EffectRunner,
  plane: ControlPlaneService,
  id: string,
  signal: AbortSignal,
  cors: Record<string, string>,
  keepAliveMs: number,
  agentId?: string,
): Promise<Response> => {
  let unsubscribe: Unsubscribe = () => {};
  const channel = new SseChannel({
    keepAliveMs,
    onTerminate: () => unsubscribe(),
  });
  const listener: SessionEventListener = (events: ReadonlyArray<Event>) => {
    channel.push(encodeSse(events));
  };

  const subscribed = await run(Effect.either(plane.subscribe(id, listener, agentId)));
  if (Either.isLeft(subscribed)) {
    channel.close();
    return errorResponse(subscribed.left, cors);
  }
  unsubscribe = subscribed.right;

  const stream = new ReadableStream<Uint8Array>(
    {
      start(controller) {
        channel.start(controller);
        const close = () => {
          channel.close();
          try {
            controller.close();
          } catch {
            // Already closed by the consumer.
          }
        };
        if (signal.aborted) close();
        else signal.addEventListener("abort", close, { once: true });
      },
      pull() {
        channel.onPull();
      },
      cancel() {
        channel.close();
      },
    },
    { highWaterMark: 1 },
  );

  return new Response(stream, { headers: { ...sseHeaders, ...cors } });
};

/**
 * The real store write behind `POST /api/sessions/import`: the rebuilt IR
 * session goes through the same paths as `/convert` — Cline's install writes
 * `<dataDir>/sessions/<id>/` + its index row, Devin's grafts cogs and saves
 * into the sqlite store.
 */
const defaultImportSession =
  (conv: { readonly dbPath: string; readonly clineDir: string }) =>
  (session: Session, agent: "cline" | "devin"): Effect.Effect<string, unknown> => {
    const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);
    if (agent === "cline") {
      return Effect.gen(function* () {
        const store = yield* ClineStore.ClineStore;
        return yield* store.install(session);
      }).pipe(
        Effect.provide(Layer.mergeAll(ClineStore.layer(openSessionsDb, conv.clineDir), fsLayer)),
      );
    }
    return Conversion.importSession(session).pipe(
      Effect.provide(Layer.mergeAll(SqliteStorage.layer(conv.dbPath), fsLayer)),
    );
  };

const HISTORY_ROLES: ReadonlySet<string> = new Set(["system", "user", "assistant", "tool"]);

export const createApp = (plane: ControlPlaneService, options: AppOptions = {}) => {
  const keepAliveMs = options.keepAliveMs ?? keepAliveMsFromEnv(process.env.SEPIA_SSE_KEEPALIVE_MS);
  const run: EffectRunner = options.run ?? Effect.runPromise;
  const aguiAgent = createAguiAgentHandler(plane, { keepAliveMs, run });
  const allowedOrigins =
    options.allowedOrigins === undefined ? ALLOWED_ORIGINS : new Set(options.allowedOrigins);
  // Routed through Effect's logger so lines hit both stdout and the OTLP log
  // exporter when the telemetry layer is installed.
  const logger = options.logger ?? ((line: string) => void run(Effect.logInfo(line)));
  // Without a persisted identity the node still answers /api/node — peers just
  // see a fresh id every boot, which is correct for throwaway instances.
  const node: NodeIdentity = options.node ?? {
    id: `node_${randomUUID().replaceAll("-", "").slice(0, 16)}`,
    name: hostname(),
    version: "0.0.0",
  };

  const route = async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const method = request.method.toUpperCase();
    const cors = corsHeaders(request.headers.get("origin"), allowedOrigins);

    if (method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    // Push subscriptions + a per-session listener that turns live events into
    // notifications even when no client has the session open.
    const push = options.meta !== undefined ? makePushStore(options.meta) : null;
    const pushUnsubs = new Map<string, () => void>();
    const sessionTitles = new Map<string, string>();
    const registerPushListener = (id: string, agentId?: string): void => {
      if (push === null || pushUnsubs.has(id)) return;
      void run(
        Effect.either(
          plane.subscribe(
            id,
            (events) => {
              notifyForEvents(push, id, agentId, sessionTitles.get(id) ?? id, events);
            },
            agentId,
          ),
        ),
      ).then((result) => {
        if (Either.isRight(result)) pushUnsubs.set(id, result.right);
      });
    };

    const segments = url.pathname.split("/").filter((segment) => segment.length > 0);
    // Session ids collide across agents (devin and cline mint their own), so
    // session-scoped routes accept `?agent=<id>` to scope the store lookup.
    const agentParam = url.searchParams.get("agent") ?? undefined;

    if (method === "GET" && segmentsEqual(segments, ["api", "push", "vapid"])) {
      return push === null
        ? jsonResponse({ error: "Meta store unavailable" }, 501, cors)
        : jsonResponse({ publicKey: push.publicKey }, 200, cors);
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "push", "subscribe"])) {
      if (push === null) return jsonResponse({ error: "Meta store unavailable" }, 501, cors);
      try {
        const body = await readJsonBody(request);
        if (
          !isRecord(body) ||
          typeof body.endpoint !== "string" ||
          !isRecord(body.keys) ||
          typeof body.keys.auth !== "string" ||
          typeof body.keys.p256dh !== "string"
        ) {
          return jsonResponse({ error: "Invalid push subscription" }, 400, cors);
        }
        const prefs = isRecord(body.prefs)
          ? { done: body.prefs.done !== false, permission: body.prefs.permission !== false }
          : { done: true, permission: true };
        push.upsert({
          endpoint: body.endpoint,
          keys: { auth: body.keys.auth, p256dh: body.keys.p256dh },
          prefs,
        });
        return jsonResponse({ ok: true }, 200, cors);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
    }

    if (method === "DELETE" && segmentsEqual(segments, ["api", "push", "subscribe"])) {
      if (push === null) return jsonResponse({ error: "Meta store unavailable" }, 501, cors);
      try {
        const body = await readJsonBody(request);
        if (isRecord(body) && typeof body.endpoint === "string") {
          push.remove(body.endpoint);
        }
        return jsonResponse({ ok: true }, 200, cors);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "health"])) {
      return healthResponse(run, plane, cors);
    }

    if (segments[0] === "api" && !isAuthorized(request, options.token)) {
      return unauthorizedResponse(cors);
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "agent"])) {
      const response = await aguiAgent(request);
      return new Response(response.body, {
        status: response.status,
        headers: { ...Object.fromEntries(response.headers), ...cors },
      });
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "fs"])) {
      const path = url.searchParams.get("path") ?? "";
      if (!path.startsWith("/")) {
        return jsonResponse({ error: "path must be absolute" }, 400, cors);
      }
      try {
        const dirs = readdirSync(path, { withFileTypes: true })
          .filter((entry) => entry.isDirectory())
          .map((entry) => join(path, entry.name))
          .sort()
          .slice(0, 200);
        return jsonResponse({ dirs }, 200, cors);
      } catch {
        return jsonResponse({ error: "Cannot read that directory" }, 400, cors);
      }
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "user"])) {
      const info = userInfo();
      return jsonResponse(
        {
          user: {
            username: info.username,
            homedir: info.homedir,
            shell: info.shell,
            hostname: hostname(),
            platform: process.platform,
            arch: process.arch,
          },
        },
        200,
        cors,
      );
    }

    // Node metadata — the one endpoint every federated client hits first.
    if (method === "GET" && segmentsEqual(segments, ["api", "node"])) {
      return jsonResponse(
        {
          id: node.id,
          name: node.name,
          version: node.version,
          protocol: PROTOCOL_VERSION,
          agents: plane.listAgents().map((agent) => agent.id),
          capabilities: ["sessions", "projects", "push", "events"],
        },
        200,
        cors,
      );
    }

    if (segments[0] === "api" && segments[1] === "servers") {
      const store = options.servers;
      const tunnels = options.tunnels;
      if (store === undefined || tunnels === undefined) {
        return jsonResponse(
          { error: "Server management is not configured on this server" },
          501,
          cors,
        );
      }
      const handled = await handleServersRoute(request, segments.slice(2), {
        store,
        tunnels,
        cors,
      });
      return handled ?? jsonResponse({ error: "Not found" }, 404, cors);
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "agents"])) {
      return jsonResponse({ agents: plane.listAgents() }, 200, cors);
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "sessions"])) {
      const withLocks = url.searchParams.get("withLocks") === "1";
      return respond(run, plane.listSessions({ withLocks }), cors, {
        shape: (sessions) => {
          const overlaid = sessions.map((session) => {
            sessionTitles.set(session.id, session.title);
            const meta = options.meta?.of(session.id);
            return {
              ...session,
              title: meta?.title ?? session.title,
              pinned: meta?.pinned ?? false,
              archived: meta?.archived ?? false,
              projectIds: meta?.projectIds ?? [],
              model: meta?.model ?? null,
              spans: meta?.spans ?? [],
            };
          });
          // Sessions created via POST /api/sessions but not yet flushed into
          // the agent's store survive restarts only in the meta file —
          // surface them so they stay reachable.
          const known = new Set(sessions.map((session) => session.id));
          const pending = Object.entries(options.meta?.sessions() ?? {}).flatMap(([id, meta]) =>
            known.has(id) || typeof meta.agent !== "string" || typeof meta.cwd !== "string"
              ? []
              : [
                  {
                    id,
                    title: meta.title ?? "New session",
                    cwd: meta.cwd,
                    agent: meta.agent,
                    updatedAt: meta.createdAt ?? new Date().toISOString(),
                    locked: false,
                    lockHolderPid: null,
                    source: "sepia",
                    busy: false,
                    pinned: meta.pinned ?? false,
                    archived: meta.archived ?? false,
                    projectIds: meta.projectIds ?? [],
                    model: meta.model ?? null,
                    spans: meta.spans ?? [],
                  },
                ],
          );
          return { sessions: [...overlaid, ...pending] };
        },
        span: "http.get /api/sessions",
      });
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "sessions"])) {
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const cwd = body.cwd;
      if (typeof cwd !== "string" || cwd.trim() === "") {
        return jsonResponse({ error: "cwd is required" }, 400, cors);
      }
      const agentId = body.agent;
      if (agentId !== undefined && typeof agentId !== "string") {
        return jsonResponse({ error: "agent must be a string" }, 400, cors);
      }
      const title = body.title;
      if (title !== undefined && typeof title !== "string") {
        return jsonResponse({ error: "title must be a string" }, 400, cors);
      }
      const model = body.model;
      if (model !== undefined && typeof model !== "string") {
        return jsonResponse({ error: "model must be a string" }, 400, cors);
      }
      const fallbacks = body.fallbacks;
      if (
        fallbacks !== undefined &&
        !(Array.isArray(fallbacks) && fallbacks.every((f) => typeof f === "string"))
      ) {
        return jsonResponse({ error: "fallbacks must be an array of strings" }, 400, cors);
      }
      const created = plane
        .createSession({
          cwd,
          agentId,
          title,
          model,
          fallbacks: fallbacks as string[] | undefined,
        })
        .pipe(
          Effect.tap(({ id, agentId: createdAgent }) =>
            Effect.sync(() => {
              // The agent may not flush the session to its store until the
              // first prompt; keep enough meta to identify it after a restart.
              registerPushListener(id, createdAgent);
              options.meta?.patch(id, {
                agent: createdAgent,
                cwd,
                createdAt: new Date().toISOString(),
                ...(model !== undefined ? { model } : {}),
                ...(title !== undefined ? { title } : {}),
              });
            }),
          ),
        );
      return respond(run, created, cors, {
        status: 201,
        span: "http.post /api/sessions",
      });
    }

    // Convert-with-explicit-IR: replays the flattened history of a session —
    // possibly fetched from a peer node — into one of this node's agent
    // stores (docs/protocol.md "Resume on…").
    if (method === "POST" && segmentsEqual(segments, ["api", "sessions", "import"])) {
      const conv = options.convert;
      if (conv === undefined) {
        return jsonResponse({ error: "Import is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const target = body.agent;
      if (target !== "cline" && target !== "devin") {
        return jsonResponse({ error: "agent must be 'cline' or 'devin'" }, 400, cors);
      }
      const cwd = body.cwd;
      if (cwd !== undefined && (typeof cwd !== "string" || cwd.trim() === "")) {
        return jsonResponse({ error: "cwd must be a non-empty string" }, 400, cors);
      }
      const title = body.title;
      if (title !== undefined && typeof title !== "string") {
        return jsonResponse({ error: "title must be a string" }, 400, cors);
      }
      const model = body.model;
      if (model !== undefined && typeof model !== "string") {
        return jsonResponse({ error: "model must be a string" }, 400, cors);
      }
      const history = body.history;
      if (!Array.isArray(history) || history.length === 0) {
        return jsonResponse({ error: "history must be a non-empty array of messages" }, 400, cors);
      }
      const messages: Conversion.ImportedHistoryMessage[] = [];
      for (const item of history) {
        if (
          !isRecord(item) ||
          typeof item.role !== "string" ||
          !HISTORY_ROLES.has(item.role) ||
          typeof item.content !== "string" ||
          typeof item.createdAt !== "number" ||
          !Number.isFinite(item.createdAt) ||
          (item.toolName !== undefined && typeof item.toolName !== "string")
        ) {
          return jsonResponse(
            { error: "history items must be { role, content, createdAt, toolName? } messages" },
            400,
            cors,
          );
        }
        messages.push({
          role: item.role as Conversion.ImportedHistoryMessage["role"],
          content: item.content,
          createdAt: item.createdAt,
          ...(typeof item.toolName === "string" ? { toolName: item.toolName } : {}),
        });
      }

      const firstUser = messages.find((message) => message.role === "user");
      const session = Conversion.sessionFromHistory({
        id: randomUUID(),
        title:
          title ?? (firstUser !== undefined ? firstUser.content.slice(0, 80) : "Imported session"),
        cwd: typeof cwd === "string" ? cwd.trim() : process.cwd(),
        model: model ?? "sepia-import",
        history: messages,
      });

      const executor = options.importSession ?? defaultImportSession(conv);
      const importedAt = Date.now();
      const runSpan = { at: importedAt, agent: target, node: node.id };
      const asControl = executor(session, target).pipe(
        Effect.tap((sessionId) =>
          Effect.sync(() => {
            // Provenance: the imported copy's run continues under `target` on
            // this node — same record an attach would write.
            options.meta?.addSpan(sessionId, runSpan);
          }),
        ),
        Effect.mapError(
          (error) =>
            new ControlError({
              code: "internal",
              message: errorMessage(error),
              cause: error,
            }),
        ),
      );
      return respond(run, asControl, cors, {
        status: 201,
        shape: (sessionId) => ({
          id: sessionId,
          title: session.title,
          cwd: session.workingDirectory,
          agent: target,
          updatedAt: new Date(session.lastActivityAt * 1000).toISOString(),
          locked: false,
          lockHolderPid: null,
          source: target,
          busy: false,
          spans: options.meta === undefined ? [] : [runSpan],
        }),
        span: "http.post /api/sessions/import",
      });
    }

    if (
      method === "PATCH" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Rename is not configured on this server" }, 501, cors);
      }
      const id = decodeURIComponent(segments[2] ?? "");
      let patchBody: unknown;
      try {
        patchBody = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(patchBody)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const patch: Record<string, unknown> = {};
      if ("title" in patchBody) {
        const title = patchBody.title;
        if (typeof title !== "string" || title.trim() === "" || title.length > 200) {
          return jsonResponse({ error: "title must be a non-empty string (max 200)" }, 400, cors);
        }
        patch.title = title.trim();
      }
      if ("pinned" in patchBody) {
        if (typeof patchBody.pinned !== "boolean") {
          return jsonResponse({ error: "pinned must be a boolean" }, 400, cors);
        }
        patch.pinned = patchBody.pinned;
      }
      if ("archived" in patchBody) {
        if (typeof patchBody.archived !== "boolean") {
          return jsonResponse({ error: "archived must be a boolean" }, 400, cors);
        }
        patch.archived = patchBody.archived;
      }
      if ("projectIds" in patchBody) {
        const projectIds = patchBody.projectIds;
        if (!Array.isArray(projectIds) || !projectIds.every((p) => typeof p === "string")) {
          return jsonResponse({ error: "projectIds must be an array of strings" }, 400, cors);
        }
        patch.projectIds = projectIds;
      }
      if ("model" in patchBody) {
        const model = patchBody.model;
        if (model !== null && (typeof model !== "string" || model.length > 100)) {
          return jsonResponse({ error: "model must be a string or null" }, 400, cors);
        }
        patch.model = model;
      }
      if (Object.keys(patch).length === 0) {
        return jsonResponse({ error: "Nothing to patch" }, 400, cors);
      }
      meta.patch(id, patch);
      return jsonResponse({ ok: true }, 200, cors);
    }

    // Keys with secrets/blobs the API must not expose (VAPID pair, push subs).
    const INTERNAL_CONFIG = new Set(["vapid", "pushSubscriptions"]);
    const publicConfig = (): Record<string, unknown> =>
      Object.fromEntries(
        Object.entries(options.meta?.config() ?? {}).filter(([key]) => !INTERNAL_CONFIG.has(key)),
      );

    if (method === "GET" && segmentsEqual(segments, ["api", "config"])) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      return jsonResponse({ config: publicConfig() }, 200, cors);
    }

    if (
      method === "PATCH" &&
      segments[0] === "api" &&
      segments[1] === "config" &&
      segments.length === 3
    ) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const key = decodeURIComponent(segments[2] ?? "");
      if (INTERNAL_CONFIG.has(key)) {
        return jsonResponse({ error: "Config key is internal" }, 400, cors);
      }
      meta.setConfig(key, isRecord(body) ? body.value : undefined);
      return jsonResponse({ key, value: isRecord(body) ? body.value : undefined }, 200, cors);
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "projects"])) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      return jsonResponse({ projects: meta.listProjects() }, 200, cors);
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "projects"])) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const name = isRecord(body) ? body.name : undefined;
      if (typeof name !== "string" || name.trim() === "" || name.length > 100) {
        return jsonResponse({ error: "name must be a non-empty string (max 100)" }, 400, cors);
      }
      return jsonResponse({ project: meta.createProject(name.trim()) }, 201, cors);
    }

    if (
      method === "PATCH" &&
      segments[0] === "api" &&
      segments[1] === "projects" &&
      segments.length === 3
    ) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      const projectId = decodeURIComponent(segments[2] ?? "");
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const name = isRecord(body) ? body.name : undefined;
      if (typeof name !== "string" || name.trim() === "" || name.length > 100) {
        return jsonResponse({ error: "name must be a non-empty string (max 100)" }, 400, cors);
      }
      if (!meta.renameProject(projectId, name.trim())) {
        return jsonResponse({ error: "Unknown project" }, 404, cors);
      }
      return jsonResponse({ ok: true }, 200, cors);
    }

    if (
      method === "DELETE" &&
      segments[0] === "api" &&
      segments[1] === "projects" &&
      segments.length === 3
    ) {
      const meta = options.meta;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      meta.deleteProject(decodeURIComponent(segments[2] ?? ""));
      return jsonResponse({ ok: true }, 200, cors);
    }

    if (
      method === "POST" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments[3] === "convert" &&
      segments.length === 4
    ) {
      const conv = options.convert;
      if (conv === undefined) {
        return jsonResponse({ error: "Convert is not configured on this server" }, 501, cors);
      }
      const id = decodeURIComponent(segments[2] ?? "");
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const target = isRecord(body) ? body.agent : undefined;
      const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);
      let effect: Effect.Effect<string, unknown>;
      if (target === "cline") {
        effect = Conversion.installCline(id, conv.clineDir).pipe(
          Effect.provide(
            Layer.mergeAll(
              Layer.provideMerge(
                ClineStore.layer(openSessionsDb, conv.clineDir),
                SqliteStorage.layerReadonly(conv.dbPath),
              ),
              fsLayer,
            ),
          ),
        );
      } else if (target === "devin") {
        effect = Conversion.importCline(join(conv.clineDir, "sessions", id)).pipe(
          Effect.provide(Layer.mergeAll(SqliteStorage.layer(conv.dbPath), fsLayer)),
        );
      } else {
        return jsonResponse({ error: "agent must be 'cline' or 'devin'" }, 400, cors);
      }
      const asControl = effect.pipe(
        Effect.mapError(
          (error) =>
            new ControlError({
              code: "internal",
              message: errorMessage(error),
              cause: error,
            }),
        ),
      );
      return respond(run, asControl, cors, {
        shape: (sessionId) => ({ sessionId }),
        span: "http.post /api/sessions/:id/convert",
      });
    }

    if (
      method === "DELETE" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const id = decodeURIComponent(segments[2] ?? "");
      const deletion = plane.deleteSession(id, { agentId: agentParam }).pipe(
        // Created-but-unflushed sessions aren't in the repo — deleting them is
        // still a success: the meta record below is all that references them.
        Effect.catchIf(
          (error) => error.code === "not_found",
          () => Effect.void,
        ),
        Effect.tap(() => Effect.sync(() => options.meta?.remove(id))),
      );
      return respond(run, deletion, cors, {
        shape: () => ({ ok: true }),
        span: "http.delete /api/sessions/:id",
      });
    }

    if (segments[0] === "api" && segments[1] === "sessions" && segments.length === 4) {
      const id = decodeURIComponent(segments[2] ?? "");
      const action = segments[3];

      if (method === "GET" && action === "history") {
        const rawLimit = url.searchParams.get("limit");
        let limit: number | undefined;
        if (rawLimit !== null && rawLimit !== "") {
          const value = Number(rawLimit);
          if (!Number.isInteger(value) || value < 0) {
            return jsonResponse({ error: "limit must be a non-negative integer" }, 400, cors);
          }
          limit = value;
        }
        const rawBefore = url.searchParams.get("before");
        let before: number | undefined;
        if (rawBefore !== null && rawBefore !== "") {
          const value = Number(rawBefore);
          if (!Number.isInteger(value) || value < 0) {
            return jsonResponse({ error: "before must be a non-negative integer" }, 400, cors);
          }
          before = value;
        }
        const historyOptions =
          limit === undefined && before === undefined && agentParam === undefined
            ? undefined
            : { limit, before, agentId: agentParam };
        return respond(run, plane.getHistory(id, historyOptions), cors, {
          span: "http.get /api/sessions/:id/history",
        });
      }

      if (method === "GET" && action === "stream") {
        return streamResponse(run, plane, id, request.signal, cors, keepAliveMs, agentParam);
      }

      if (method === "POST" && action === "attach") {
        let takeover = false;
        let model: string | undefined;
        let fallbacks: ReadonlyArray<string> | undefined;
        try {
          const body = await readJsonBody(request);
          if (body !== undefined) {
            if (!isRecord(body)) {
              return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
            }
            takeover = body.takeover === true;
            if (typeof body.model === "string") model = body.model;
            if (
              Array.isArray(body.fallbacks) &&
              body.fallbacks.every((f: unknown) => typeof f === "string")
            ) {
              fallbacks = body.fallbacks as string[];
            }
          }
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        return respond(
          run,
          plane.attach(id, { takeover, model, fallbacks, agentId: agentParam }).pipe(
            Effect.tap((result) =>
              Effect.sync(() => {
                registerPushListener(id, agentParam);
                // Provenance: an attach means the run continues under this
                // node's control plane — record which agent + node own the
                // span. Idempotent, so a same-agent re-attach doesn't dup.
                if (result.attached) {
                  options.meta?.addSpan(id, {
                    at: Date.now(),
                    agent: result.agentId,
                    node: node.id,
                  });
                }
              }),
            ),
          ),
          cors,
          {
            span: "http.post /api/sessions/:id/attach",
          },
        );
      }

      if (method === "POST" && action === "prompt") {
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        if (!isRecord(body) || typeof body.text !== "string" || body.text.trim() === "") {
          return jsonResponse({ error: "text is required" }, 400, cors);
        }
        return respond(run, plane.prompt(id, body.text, agentParam), cors, {
          shape: () => ({ ok: true }),
          span: "http.post /api/sessions/:id/prompt",
        });
      }

      if (method === "POST" && action === "cancel") {
        return respond(run, plane.cancel(id, agentParam), cors, {
          shape: () => ({ ok: true }),
          span: "http.post /api/sessions/:id/cancel",
        });
      }

      if (method === "POST" && action === "permission") {
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        if (!isRecord(body) || typeof body.requestId !== "string") {
          return jsonResponse({ error: "requestId is required" }, 400, cors);
        }
        const optionId = body.optionId === undefined ? null : body.optionId;
        if (optionId !== null && typeof optionId !== "string") {
          return jsonResponse({ error: "optionId must be a string or null" }, 400, cors);
        }
        return respond(
          run,
          plane.respondToPermission(id, body.requestId, optionId, agentParam),
          cors,
          {
            span: "http.post /api/sessions/:id/permission",
            shape: () => ({ ok: true }),
          },
        );
      }
    }

    return jsonResponse({ error: "Not found" }, 404, cors);
  };

  return async (request: Request): Promise<Response> => {
    const started = performance.now();
    const response = await route(request);
    const path = new URL(request.url).pathname;
    if (path !== "/api/health") {
      const duration = Math.round(performance.now() - started);
      logger(`${request.method.toUpperCase()} ${path} ${response.status} ${duration}ms`);
    }
    return response;
  };
};
