import { readdirSync } from "node:fs";
import { join } from "node:path";
import { jsonResponse, segmentsEqual, type RouteHandler } from "./shared";

/**
 * `GET /api/fs?path=/abs/dir` — direct children of a directory, for the
 * composer's working-directory picker. Directories only, capped at 200.
 */
export const createFsRoute =
  (): RouteHandler =>
  ({ url, method, segments, cors }) => {
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
    return undefined;
  };
