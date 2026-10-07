import type { PushStore } from "../push";
import { isRecord, jsonResponse, readJsonBody, segmentsEqual, type RouteHandler } from "./shared";

export interface PushRouteDeps {
  /** `null` when no meta store is configured — every route answers 501. */
  readonly push: PushStore | null;
}

/**
 * Push subscription management — authenticated like every other /api
 * route: an open subscribe endpoint would let anyone on the network
 * register their own endpoint and receive notification payloads.
 */
export const createPushRoute =
  (deps: PushRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    const { push } = deps;

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

    return undefined;
  };
