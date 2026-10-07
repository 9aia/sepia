import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { Effect, Either, Layer } from "effect";
import { Session } from "sepia-core";
import { Conversion, ClineStore } from "sepia-convert";
import { openSessionsDb, SqliteStorage } from "sepia-devin";
import { ControlError } from "sepia-session-control";
import type { ControlErrorCode } from "sepia-session-control";

export type EffectRunner = <A, E>(effect: Effect.Effect<A, E>) => Promise<A>;

/**
 * Per-request context handed to every route handler — the pieces `route()`
 * used to close over: the parsed URL, upper-cased method, split path
 * segments, the CORS headers computed for this request, and `?agent` (the
 * disambiguator for session ids that collide across agent stores).
 */
export interface RouteContext {
  readonly request: Request;
  readonly url: InstanceType<typeof URL>;
  readonly method: string;
  readonly segments: ReadonlyArray<string>;
  readonly cors: Record<string, string>;
  readonly agentParam: string | undefined;
}

/**
 * A route module's entry point: returns the response when its path/method
 * matched, `undefined` to fall through to the next table entry.
 */
export type RouteHandler = (
  ctx: RouteContext,
) => Promise<Response | undefined> | Response | undefined;

export const CODE_STATUS: Readonly<Record<ControlErrorCode, number>> = {
  not_found: 404,
  invalid: 400,
  unknown_agent: 400,
  locked: 409,
  conflict: 409,
  busy: 409,
  internal: 500,
};

export const jsonResponse = (
  body: unknown,
  status: number,
  cors: Record<string, string>,
): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...cors },
  });

export const errorMessage = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { readonly message: unknown }).message);
  }
  return String(error);
};

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export const errorCode = (error: unknown): ControlErrorCode | undefined => {
  if (!isRecord(error)) return undefined;
  const code = error.code;
  return typeof code === "string" && code in CODE_STATUS ? (code as ControlErrorCode) : undefined;
};

export const errorResponse = (error: unknown, cors: Record<string, string>): Response => {
  const message = errorMessage(error);
  const code = errorCode(error);
  if (code === undefined) return jsonResponse({ error: message }, 500, cors);
  return jsonResponse({ error: message, code }, CODE_STATUS[code], cors);
};

export const respond = async <A>(
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

export const readJsonBody = async (request: Request): Promise<unknown> => {
  const text = await request.text();
  if (text.trim() === "") return undefined;
  return JSON.parse(text) as unknown;
};

export const segmentsEqual = (
  segments: ReadonlyArray<string>,
  pattern: ReadonlyArray<string>,
): boolean =>
  segments.length === pattern.length && pattern.every((part, index) => part === segments[index]);

/**
 * The real store write behind `POST /api/sessions/import`: the rebuilt IR
 * session goes through the same paths as `/convert` — Cline's install writes
 * `<dataDir>/sessions/<id>/` + its index row, Devin's grafts cogs and saves
 * into the sqlite store. Shared by the session-import route and the project
 * transfer verbs (their session writes reuse this executor).
 */
export const defaultImportSession =
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
