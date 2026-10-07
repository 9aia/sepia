import type { MetaStore } from "../meta";
import { isRecord, jsonResponse, readJsonBody, segmentsEqual, type RouteHandler } from "./shared";

// Keys with secrets/blobs the API must not expose (VAPID pair, push subs).
const INTERNAL_CONFIG = new Set(["vapid", "pushSubscriptions"]);

export interface ConfigRouteDeps {
  readonly metaStore: MetaStore | undefined;
}

export const createConfigRoute =
  (deps: ConfigRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    const metaStore = deps.metaStore;
    const publicConfig = (): Record<string, unknown> =>
      Object.fromEntries(
        Object.entries(metaStore?.config() ?? {}).filter(([key]) => !INTERNAL_CONFIG.has(key)),
      );

    if (method === "GET" && segmentsEqual(segments, ["api", "config"])) {
      const meta = metaStore;
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
      const meta = metaStore;
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

    return undefined;
  };
