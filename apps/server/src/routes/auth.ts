import type { Pairing } from "../pair";
import { isRecord, jsonResponse, readJsonBody, segmentsEqual, type RouteHandler } from "./shared";

export interface AuthRouteDeps {
  /** The env bearer token — same value isAuthorized checks. */
  readonly token: string | undefined;
  readonly pairing: Pairing | undefined;
  /** Presented-token verifier shared with the bearer gate. */
  readonly accepts: (provided: string) => boolean;
}

/** Cookie name — value is the credential itself (env token or paired). */
export const AUTH_COOKIE = "sepia_token";

/**
 * `POST /api/auth/login` — exchanges a presented token for an httpOnly
 * cookie, so the browser token never touches localStorage (or JS at all).
 * Deliberately unauthenticated — this IS the credential bootstrap, the
 * sibling of `POST /api/pair`: the presented token authorizes it.
 *
 * `POST /api/auth/logout` — expires the cookie.
 *
 * The cookie carries the *provided* credential verbatim — env token or a
 * paired credential — so `isAuthorized` validates it through the exact
 * same `tokenMatches`/`pairing.accepts` path as a Bearer header.
 * SameSite=Strict blocks ambient cross-site sends (CSRF); Secure rides
 * only on https origins so LAN http keeps working.
 */
export const createAuthRoute =
  (deps: AuthRouteDeps): RouteHandler =>
  async ({ request, method, url, segments, cors }) => {
    if (method === "POST" && segmentsEqual(segments, ["api", "auth", "login"])) {
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body) || typeof body.token !== "string" || body.token.trim() === "") {
        return jsonResponse({ error: "token is required" }, 400, cors);
      }
      const provided = body.token;
      // No auth configured → the cookie grants nothing anyway; accept so
      // the client's flow doesn't branch on deployment mode.
      const ok = deps.token === undefined || deps.accepts(provided);
      if (!ok) {
        return jsonResponse({ error: "Invalid token" }, 401, cors);
      }
      const secure = url.protocol === "https:" ? "; Secure" : "";
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: {
          "content-type": "application/json",
          "set-cookie": `${AUTH_COOKIE}=${provided}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000${secure}`,
          ...cors,
        },
      });
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "auth", "logout"])) {
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: {
          "content-type": "application/json",
          "set-cookie": `${AUTH_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0`,
          ...cors,
        },
      });
    }

    return undefined;
  };

/**
 * Read the auth cookie — `sepia_token=<credential>` — from a request.
 * Returns undefined when absent; the caller falls through to the other
 * credential paths.
 */
export const cookieToken = (request: Request): string | undefined => {
  const cookie = request.headers.get("cookie");
  if (cookie === null) return undefined;
  const match = cookie.match(new RegExp(`(?:^|;\\s*)${AUTH_COOKIE}=([^;]+)`));
  return match === null ? undefined : decodeURIComponent(match[1] ?? "");
};
