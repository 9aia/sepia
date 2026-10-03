import { createHash, timingSafeEqual } from "node:crypto";
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
import type { CopilotKitHandler } from "./copilotkit";
import { keepAliveMsFromEnv, SseChannel } from "./sse-channel";

export interface AppOptions {
  /** Mounted at `/api/copilotkit`; omitted in tests, where the route reports 501. */
  readonly copilotkitHandler?: CopilotKitHandler;
  /** When set, every `/api/*` route except `GET /api/health` requires a bearer token. */
  readonly token?: string;
  /** Receives one line per request; defaults to `console.log`. */
  readonly logger?: (line: string) => void;
  /** Overrides the built-in CORS allowlist. */
  readonly allowedOrigins?: ReadonlyArray<string>;
  /** Overrides `SEPIA_SSE_KEEPALIVE_MS`; tests use a tiny value. */
  readonly keepAliveMs?: number;
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
    headers["access-control-allow-headers"] = "content-type";
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
  if (header === null || !header.startsWith("Bearer ")) return false;
  return tokenMatches(header.slice("Bearer ".length), token);
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

const respond = async <A>(
  effect: Effect.Effect<A, ControlError>,
  cors: Record<string, string>,
  shape: (value: A) => unknown = (value) => value,
  status = 200,
): Promise<Response> => {
  // `Effect.either` keeps the raw `ControlError` (with its `code`) rather than the
  // `FiberFailure` wrapper that `runPromise` would reject with.
  const result = await Effect.runPromise(Effect.either(effect));
  if (Either.isLeft(result)) return errorResponse(result.left, cors);
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
  plane: ControlPlaneService,
  cors: Record<string, string>,
): Promise<Response> => {
  const healthy = await Effect.runPromise(
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

  const subscribed = await Effect.runPromise(Effect.either(plane.subscribe(id, listener)));
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
  const aguiAgent = createAguiAgentHandler(plane, { keepAliveMs });
  const allowedOrigins =
    options.allowedOrigins === undefined ? ALLOWED_ORIGINS : new Set(options.allowedOrigins);
  const logger = options.logger ?? ((line: string) => console.log(line));

  const route = async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const method = request.method.toUpperCase();
    const cors = corsHeaders(request.headers.get("origin"), allowedOrigins);

    if (method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    const segments = url.pathname.split("/").filter((segment) => segment.length > 0);

    if (method === "GET" && segmentsEqual(segments, ["api", "health"])) {
      return healthResponse(plane, cors);
    }

    if (segments[0] === "api" && !isAuthorized(request, options.token)) {
      return unauthorizedResponse(cors);
    }

    if (segments[0] === "api" && segments[1] === "copilotkit") {
      if (options.copilotkitHandler === undefined) {
        return jsonResponse({ error: "CopilotKit runtime is not mounted" }, 501, cors);
      }
      const response = await options.copilotkitHandler(request);
      return new Response(response.body, {
        status: response.status,
        headers: { ...Object.fromEntries(response.headers), ...cors },
      });
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "agent"])) {
      const response = await aguiAgent(request);
      return new Response(response.body, {
        status: response.status,
        headers: { ...Object.fromEntries(response.headers), ...cors },
      });
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "agents"])) {
      return jsonResponse({ agents: plane.listAgents() }, 200, cors);
    }

    if (method === "GET" && segmentsEqual(segments, ["api", "sessions"])) {
      const withLocks = url.searchParams.get("withLocks") === "1";
      return respond(plane.listSessions({ withLocks }), cors, (sessions) => ({ sessions }));
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
      return respond(plane.createSession({ cwd, agentId, title }), cors, (value) => value, 201);
    }

    if (
      method === "DELETE" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const id = decodeURIComponent(segments[2] ?? "");
      return respond(plane.deleteSession(id), cors, () => ({ ok: true }));
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
        return respond(plane.getHistory(id, limit === undefined ? undefined : { limit }), cors);
      }

      if (method === "GET" && action === "stream") {
        return streamResponse(plane, id, request.signal, cors, keepAliveMs);
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
        return respond(plane.attach(id, { takeover }), cors);
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
        return respond(plane.prompt(id, body.text), cors, () => ({ ok: true }));
      }

      if (method === "POST" && action === "cancel") {
        return respond(plane.cancel(id), cors, () => ({ ok: true }));
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
        return respond(plane.respondToPermission(id, body.requestId, optionId), cors, () => ({
          ok: true,
        }));
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
