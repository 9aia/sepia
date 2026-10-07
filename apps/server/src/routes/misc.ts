import { Effect } from "effect";
import type { ControlPlaneService } from "sepia-session-control";
import { jsonResponse, segmentsEqual, type EffectRunner, type RouteHandler } from "./shared";

const HEALTH_TIMEOUT_MS = 1_500;

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

export interface HealthRouteDeps {
  readonly run: EffectRunner;
  readonly plane: ControlPlaneService;
}

/**
 * `GET /api/health` — deliberately ahead of the auth gate (load balancers
 * and `vp env doctor`-style probes don't carry a bearer token).
 */
export const createHealthRoute =
  (deps: HealthRouteDeps): RouteHandler =>
  ({ method, segments, cors }) => {
    if (method === "GET" && segmentsEqual(segments, ["api", "health"])) {
      return healthResponse(deps.run, deps.plane, cors);
    }
    return undefined;
  };
