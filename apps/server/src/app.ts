import { createHash, timingSafeEqual } from "node:crypto";
import { readdirSync } from "node:fs";
import { hostname, userInfo } from "node:os";
import { join } from "node:path";
import { Effect, Either } from "effect";
import { encodeSse, sseHeaders, type Event } from "sepia-agui";
import type {
  ControlError,
  ControlErrorCode,
  ControlPlaneService,
  SessionEventListener,
  Unsubscribe,
} from "sepia-session-control";
import { createAguiAgentHandler } from "./agui-agent";
import type { MetaStore } from "./meta";
import { keepAliveMsFromEnv, SseChannel } from "./sse-channel";

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
  if (origin !== null && allowed.has(origin)) {
    headers["access-control-allow-origin"] = origin;
    headers["access-control-allow-methods"] = "GET,POST,OPTIONS";
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
): Promise<Response> => {
  let unsubscribe: Unsubscribe = () => {};
  const channel = new SseChannel({
    keepAliveMs,
    onTerminate: () => unsubscribe(),
  });
  const listener: SessionEventListener = (events: ReadonlyArray<Event>) => {
    channel.push(encodeSse(events));
  };

  const subscribed = await run(Effect.either(plane.subscribe(id, listener)));
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

export const createApp = (plane: ControlPlaneService, options: AppOptions = {}) => {
  const keepAliveMs = options.keepAliveMs ?? keepAliveMsFromEnv(process.env.SEPIA_SSE_KEEPALIVE_MS);
  const run: EffectRunner = options.run ?? Effect.runPromise;
  const aguiAgent = createAguiAgentHandler(plane, { keepAliveMs, run });
  const allowedOrigins =
    options.allowedOrigins === undefined ? ALLOWED_ORIGINS : new Set(options.allowedOrigins);
  // Routed through Effect's logger so lines hit both stdout and the OTLP log
  // exporter when the telemetry layer is installed.
  const logger = options.logger ?? ((line: string) => void run(Effect.logInfo(line)));

  const route = async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const method = request.method.toUpperCase();
    const cors = corsHeaders(request.headers.get("origin"), allowedOrigins);

    if (method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    const segments = url.pathname.split("/").filter((segment) => segment.length > 0);

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

    if (method === "GET" && segmentsEqual(segments, ["api", "agents"])) {
      return jsonResponse({ agents: plane.listAgents() }, 200, cors);
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "sessions"])) {
      const withLocks = url.searchParams.get("withLocks") === "1";
      return respond(run, plane.listSessions({ withLocks }), cors, {
        shape: (sessions) => ({
          sessions: sessions.map((session) => {
            const meta = options.meta?.of(session.id);
            return {
              ...session,
              title: meta?.title ?? session.title,
              pinned: meta?.pinned ?? false,
              projectId: meta?.projectId ?? null,
            };
          }),
        }),
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
      return respond(run, plane.createSession({ cwd, agentId, title }), cors, {
        status: 201,
        span: "http.post /api/sessions",
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
      if ("projectId" in patchBody) {
        const projectId = patchBody.projectId;
        if (projectId !== null && typeof projectId !== "string") {
          return jsonResponse({ error: "projectId must be a string or null" }, 400, cors);
        }
        patch.projectId = projectId;
      }
      if (Object.keys(patch).length === 0) {
        return jsonResponse({ error: "Nothing to patch" }, 400, cors);
      }
      meta.patch(id, patch);
      return jsonResponse({ ok: true }, 200, cors);
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
      method === "DELETE" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const id = decodeURIComponent(segments[2] ?? "");
      return respond(
        run,
        plane.deleteSession(id).pipe(Effect.tap(() => Effect.sync(() => options.meta?.remove(id)))),
        cors,
        {
          shape: () => ({ ok: true }),
          span: "http.delete /api/sessions/:id",
        },
      );
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
          limit === undefined && before === undefined ? undefined : { limit, before };
        return respond(run, plane.getHistory(id, historyOptions), cors, {
          span: "http.get /api/sessions/:id/history",
        });
      }

      if (method === "GET" && action === "stream") {
        return streamResponse(run, plane, id, request.signal, cors, keepAliveMs);
      }

      if (method === "POST" && action === "attach") {
        let takeover = false;
        try {
          const body = await readJsonBody(request);
          if (body !== undefined) {
            if (!isRecord(body)) {
              return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
            }
            takeover = body.takeover === true;
          }
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        return respond(run, plane.attach(id, { takeover }), cors, {
          span: "http.post /api/sessions/:id/attach",
        });
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
        return respond(run, plane.prompt(id, body.text), cors, {
          shape: () => ({ ok: true }),
          span: "http.post /api/sessions/:id/prompt",
        });
      }

      if (method === "POST" && action === "cancel") {
        return respond(run, plane.cancel(id), cors, {
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
        return respond(run, plane.respondToPermission(id, body.requestId, optionId), cors, {
          span: "http.post /api/sessions/:id/permission",
          shape: () => ({ ok: true }),
        });
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
