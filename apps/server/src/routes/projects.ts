import type { MetaStore } from "../meta";
import { isRecord, jsonResponse, readJsonBody, segmentsEqual, type RouteHandler } from "./shared";

export interface ProjectsRouteDeps {
  readonly metaStore: MetaStore | undefined;
}

/**
 * Project CRUD — named groups over session meta (`meta.projectIds`).
 * The transfer verbs live in ./transfer.ts.
 */
export const createProjectsRoute =
  (deps: ProjectsRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    const metaStore = deps.metaStore;

    if (method === "GET" && segmentsEqual(segments, ["api", "projects"])) {
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      return jsonResponse({ projects: meta.listProjects() }, 200, cors);
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "projects"])) {
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
      const meta = metaStore;
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
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      meta.deleteProject(decodeURIComponent(segments[2] ?? ""));
      return jsonResponse({ ok: true }, 200, cors);
    }

    return undefined;
  };
