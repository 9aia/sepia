import { createHash, randomUUID, timingSafeEqual } from "node:crypto";
import { hostname } from "node:os";
import { Effect, Either } from "effect";
import type { Event } from "sepia-agui";
import type { Session } from "sepia-core";
import type { ControlPlaneService, SessionSummary } from "sepia-session-control";
import { createAguiAgentHandler } from "./agui-agent";
import { busyFromEvents, createEventFeed, instrumentMeta, sessionPayload } from "./events";
import type { MetaStore } from "./meta";
import type { NodeIdentity } from "./node";
import type { Pairing } from "./pair";
import type { ServerStore } from "./servers";
import type { TunnelManager } from "./ssh";
import { keepAliveMsFromEnv } from "./sse-channel";
import type { UiAssets } from "./ui";
import { makePushStore, notifyForEvents } from "./push";
import { createAgentRoute, createAgentsRoute } from "./routes/agents";
import { createClientRoute } from "./routes/client";
import { createConfigRoute } from "./routes/config";
import { createEventsRoute } from "./routes/events";
import { createFsRoute } from "./routes/fs";
import { createHealthRoute } from "./routes/misc";
import { createNodeRoute } from "./routes/node";
import { createPairRoute } from "./routes/pair";
import { cookieToken, createAuthRoute } from "./routes/auth";
import { createProjectsRoute } from "./routes/projects";
import { createPushRoute } from "./routes/push";
import { createServersRoute } from "./routes/servers";
import { createSessionsRoute } from "./routes/sessions";
import { createTransferRoute } from "./routes/transfer";
import {
  defaultImportSession,
  errorMessage,
  jsonResponse,
  type RouteContext,
  type RouteHandler,
} from "./routes/shared";

export type { EffectRunner } from "./routes/shared";
import type { EffectRunner } from "./routes/shared";

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
  /**
   * Overrides `SEPIA_HELD_WATCH_MS` — how often the held-session watch
   * re-probes lock state for sessions last seen read-only; `0` disables.
   */
  readonly heldWatchMs?: number;
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
   * Test seam for the project transfer verbs (pull/push fetch the peer
   * node-to-node). Defaults to globalThis.fetch.
   */
  readonly fetchImpl?: typeof fetch;
  /**
   * Node identity reported by `GET /api/node` (docs/protocol.md). Absent → an
   * ephemeral id is minted for the process lifetime (tests, embedded use).
   */
  readonly node?: NodeIdentity;
  /** Managed-server registry; absent → /api/servers returns 501. */
  readonly servers?: ServerStore;
  /** SSH tunnel manager backing ssh-enabled registry entries. */
  readonly tunnels?: TunnelManager;
  /**
   * Pairing backend (docs/protocol.md): `POST /api/pair` redeems a one-time
   * code and issued credentials authenticate like `SEPIA_TOKEN`. Absent →
   * `POST /api/pair` returns 501.
   */
  readonly pairing?: Pairing;
  /**
   * Static web UI (the built TanStack Start SPA). When set, GET/HEAD requests
   * outside /api/* fall through to it — extensionless paths get index.html.
   */
  readonly ui?: UiAssets;
}

const ALLOWED_ORIGINS: ReadonlySet<string> = new Set([
  "http://localhost:3000",
  "http://127.0.0.1:3000",
]);

const DEFAULT_HELD_WATCH_MS = 5_000;
// A held session stops being watched this long after the last read-only
// attach observed it — a closed tab can't leave the probe running forever.
const HELD_WATCH_TTL_MS = 30 * 60_000;

/** `SEPIA_HELD_WATCH_MS=0` disables the held-session watch. */
const heldWatchMsFromEnv = (raw: string | undefined): number => {
  if (raw === undefined || raw === "") return DEFAULT_HELD_WATCH_MS;
  const value = Number(raw);
  return Number.isFinite(value) && value >= 0 ? Math.floor(value) : DEFAULT_HELD_WATCH_MS;
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

// Hash both sides so the buffers are always equal length; a raw length mismatch
// would make timingSafeEqual throw and leak the expected token's length.
const tokenMatches = (provided: string, expected: string): boolean =>
  timingSafeEqual(
    createHash("sha256").update(provided).digest(),
    createHash("sha256").update(expected).digest(),
  );

/**
 * `?access_token` exists only because EventSource can't set headers, so the
 * query credential is honored on just the two SSE GET routes — the node
 * event feed and the per-session stream — including under their
 * `/api/gateway/:id` and `/api/servers/:id/proxy` mounts. Everywhere else
 * needs the bearer header: keeping query auth off non-SSE paths shrinks the
 * surface where a token can end up persisted in a URL (browser history,
 * proxy and access logs).
 */
const QUERY_TOKEN_PATH = /\/api\/(?:events|sessions\/[^/]+\/stream)$/;

const isAuthorized = (request: Request, token: string | undefined, pairing?: Pairing): boolean => {
  if (token === undefined || token === "") return true;
  const header = request.headers.get("authorization");
  let provided = header?.startsWith("Bearer ") ? header.slice("Bearer ".length) : null;
  // HttpOnly-cookie auth (POST /api/auth/login): same credential, held
  // outside JS reach — Bearer stays authoritative when both ride.
  if (provided === null) {
    provided = cookieToken(request) ?? null;
  }
  if (provided === null && request.method === "GET") {
    // The access log only records url.pathname, never query params.
    const url = new URL(request.url);
    if (QUERY_TOKEN_PATH.test(url.pathname)) {
      provided = url.searchParams.get("access_token");
    }
  }
  if (provided === null || provided === undefined) return false;
  // Paired credentials are checked by hash — equivalent privilege to the
  // env token, but revocable-by-file-deletion and never stored in plaintext.
  return tokenMatches(provided, token) || pairing?.accepts(provided) === true;
};

const unauthorizedResponse = (cors: Record<string, string>): Response =>
  new Response(JSON.stringify({ error: "Unauthorized" }), {
    status: 401,
    headers: { "content-type": "application/json", "www-authenticate": "Bearer", ...cors },
  });

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

  // The node event feed (docs/protocol.md): one emitter per app, drained by
  // each GET /api/events connection. The meta overlay is instrumented so
  // every title/pin/project write emits meta/project events automatically.
  const feed = createEventFeed();
  const metaStore = options.meta === undefined ? undefined : instrumentMeta(options.meta, feed);

  const heldWatchMs = options.heldWatchMs ?? heldWatchMsFromEnv(process.env.SEPIA_HELD_WATCH_MS);

  /**
   * Held-session watch (docs/protocol.md): a read-only attach means another
   * process owns the session's store lock — the holder's transcript writes
   * and its eventual release happen outside this node, so no local emit ever
   * reports them. While a session is known held AND the feed has listeners,
   * the watch re-lists sessions with locks (`withLocks` shares the control
   * plane's probe TTL, so ticks inside the TTL cost no extra agent spawns)
   * and diffs each watched summary: a `locked` flip or an `updatedAt` bump
   * (the holder flushing transcript rows) becomes the `session` events the
   * web UI used to poll `GET /api/sessions?withLocks=1` for.
   */
  interface HeldWatch {
    readonly id: string;
    /** Owning agent when the attach resolved one — disambiguates colliding ids. */
    readonly agent?: string;
    /** Last attach that saw the session held; refreshes HELD_WATCH_TTL_MS. */
    at: number;
  }
  const heldWatches = new Map<string, HeldWatch>();
  // The diff baseline, seeded `{locked: true}` so the first tick reports
  // either the holder details or the release edge.
  const heldBaseline = new Map<string, Partial<SessionSummary>>();
  let heldTimer: ReturnType<typeof setInterval> | undefined;

  const HELD_FIELDS = ["title", "updatedAt", "locked", "lockHolderPid", "busy"] as const;

  const tickHeldWatches = async (): Promise<void> => {
    const now = Date.now();
    for (const [key, watch] of heldWatches) {
      if (now - watch.at > HELD_WATCH_TTL_MS) {
        heldWatches.delete(key);
        heldBaseline.delete(key);
      }
    }
    if (heldWatches.size === 0) {
      if (heldTimer !== undefined) {
        clearInterval(heldTimer);
        heldTimer = undefined;
      }
      return;
    }
    // Nobody drains the feed — the probe (agent spawns) buys nothing.
    if (feed.subscriberCount() === 0) return;
    const listed = await run(Effect.either(plane.listSessions({ withLocks: true })));
    if (Either.isLeft(listed)) {
      void run(Effect.logWarning(`held-session watch failed: ${errorMessage(listed.left)}`));
      return;
    }
    for (const [key, watch] of heldWatches) {
      const summary = listed.right.find(
        (row) => row.id === watch.id && (watch.agent === undefined || row.agent === watch.agent),
      );
      if (summary === undefined) {
        // The store row vanished while held — same shape as DELETE.
        feed.emit("session", sessionPayload(watch.id, watch.agent, { deleted: true }));
        heldWatches.delete(key);
        heldBaseline.delete(key);
        continue;
      }
      const prev = heldBaseline.get(key);
      const patch: Record<string, unknown> = {};
      for (const field of HELD_FIELDS) {
        if (prev === undefined || summary[field] !== prev[field]) {
          patch[field] = summary[field];
        }
      }
      heldBaseline.set(
        key,
        Object.fromEntries(HELD_FIELDS.map((field) => [field, summary[field]])),
      );
      if (Object.keys(patch).length > 0) {
        feed.emit("session", sessionPayload(summary.id, summary.agent, patch));
      }
    }
  };

  // Keys carry the agent — session ids collide across agent stores.
  const watchHeld = (id: string, agent?: string): void => {
    const key = `${agent ?? ""}:${id}`;
    heldWatches.set(key, { id, agent, at: Date.now() });
    heldBaseline.set(key, { locked: true });
    if (heldTimer === undefined && heldWatchMs > 0) {
      heldTimer = setInterval(() => void tickHeldWatches(), heldWatchMs);
      (heldTimer as { unref?: () => void }).unref?.();
    }
  };

  const unwatchHeld = (id: string, agent?: string): void => {
    for (const [key, watch] of heldWatches) {
      if (watch.id === id && (agent === undefined || watch.agent === agent)) {
        heldWatches.delete(key);
        heldBaseline.delete(key);
      }
    }
  };

  // Push subscriptions + a per-session listener that turns live AG-UI events
  // into notifications and `busy` feed events even when no client has the
  // session open. These live at app scope — per-request maps used to leak a
  // fresh plane.subscribe() on every attach.
  const push = metaStore !== undefined ? makePushStore(metaStore) : null;
  const liveUnsubs = new Map<string, () => void>();
  const sessionTitles = new Map<string, string>();
  const registerLiveListener = (id: string, agentId?: string): void => {
    if (liveUnsubs.has(id)) return;
    void run(
      Effect.either(
        plane.subscribe(
          id,
          (events: ReadonlyArray<Event>) => {
            if (push !== null) {
              notifyForEvents(push, id, agentId, sessionTitles.get(id) ?? id, events);
            }
            const busy = busyFromEvents(events);
            if (busy !== undefined) {
              feed.emit("session", sessionPayload(id, agentId, { busy }));
            }
          },
          agentId,
        ),
      ),
    ).then((result) => {
      if (Either.isRight(result)) liveUnsubs.set(id, result.right);
    });
  };

  // Session writes for the transfer verbs reuse the /api/sessions/import
  // executor, so toolCalls, thinking, usage and checkpoints survive the hop.
  const transferExecutor =
    options.importSession ??
    (options.convert === undefined ? undefined : defaultImportSession(options.convert));

  // Pre-auth routes — checked in order, ahead of the bearer gate:
  // health (unauthenticated for probes) and pairing (the credential
  // bootstrap itself).
  const preAuth: ReadonlyArray<RouteHandler> = [
    createHealthRoute({ run, plane }),
    createPairRoute({ pairing: options.pairing }),
    createAuthRoute({
      token: options.token,
      pairing: options.pairing,
      accepts: (provided) =>
        options.token !== undefined &&
        (tokenMatches(provided, options.token) || options.pairing?.accepts(provided) === true),
    }),
  ];

  // Post-auth route table — dispatch order mirrors the original if-chain in
  // route(): overlapping shapes (e.g. POST /api/sessions/import vs
  // /api/sessions/:id/*) resolve exactly as before.
  const routes: ReadonlyArray<RouteHandler> = [
    createPushRoute({ push }),
    createAgentRoute({ aguiAgent }),
    createFsRoute(),
    createNodeRoute({ plane, node, pairing: options.pairing !== undefined }),
    createClientRoute(),
    createServersRoute({ servers: options.servers, tunnels: options.tunnels }),
    createAgentsRoute({ plane }),
    createEventsRoute({ feed, keepAliveMs }),
    createSessionsRoute({
      run,
      plane,
      feed,
      metaStore,
      node,
      keepAliveMs,
      convert: options.convert,
      importSession: options.importSession,
      sessionTitles,
      registerLiveListener,
      watchHeld,
      unwatchHeld,
    }),
    createConfigRoute({ metaStore }),
    createProjectsRoute({ metaStore }),
    createTransferRoute({
      run,
      plane,
      metaStore,
      feed,
      node,
      keepAliveMs,
      executor: transferExecutor,
      fetchImpl: options.fetchImpl,
    }),
  ];

  const route = async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const method = request.method.toUpperCase();
    const cors = corsHeaders(request.headers.get("origin"), allowedOrigins);

    if (method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors });
    }

    const segments = url.pathname.split("/").filter((segment) => segment.length > 0);
    // Session ids collide across agents (devin and cline mint their own), so
    // session-scoped routes accept `?agent=<id>` to scope the store lookup.
    const agentParam = url.searchParams.get("agent") ?? undefined;
    const ctx: RouteContext = { request, url, method, segments, cors, agentParam };

    for (const handler of preAuth) {
      const response = await handler(ctx);
      if (response !== undefined) return response;
    }

    if (segments[0] === "api" && !isAuthorized(request, options.token, options.pairing)) {
      return unauthorizedResponse(cors);
    }

    for (const handler of routes) {
      const response = await handler(ctx);
      if (response !== undefined) return response;
    }

    // Non-API GETs fall through to the bundled SPA — this same-origin host is
    // what lets the binary serve UI + API on one port (docs/DEPLOY.md).
    if (
      options.ui !== undefined &&
      (method === "GET" || method === "HEAD") &&
      segments[0] !== "api"
    ) {
      const served = await options.ui.fetch(method, url.pathname);
      if (served !== null) return served;
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
