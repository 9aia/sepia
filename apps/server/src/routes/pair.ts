import type { Pairing } from "../pair";
import { isRecord, jsonResponse, readJsonBody, segmentsEqual, type RouteHandler } from "./shared";

export interface PairRouteDeps {
  readonly pairing: Pairing | undefined;
}

/**
 * `POST /api/pair` — pairing (docs/protocol.md): deliberately
 * unauthenticated — this IS the credential bootstrap. The code, not a
 * bearer token, authorizes it, so the route table registers it ahead of
 * the auth gate.
 */
export const createPairRoute =
  (deps: PairRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    if (method === "POST" && segmentsEqual(segments, ["api", "pair"])) {
      const pairing = deps.pairing;
      if (pairing === undefined) {
        return jsonResponse({ error: "Pairing is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body) || typeof body.code !== "string" || body.code.trim() === "") {
        return jsonResponse({ error: "code is required" }, 400, cors);
      }
      const token = pairing.redeem(body.code);
      // One 404 for unknown/expired/used — don't leak which case it was.
      if (token === null) {
        return jsonResponse({ error: "Invalid or expired pairing code" }, 404, cors);
      }
      return jsonResponse({ token }, 200, cors);
    }
    return undefined;
  };
