import type { NodeIdentity } from "../node";
import type { MetaStore } from "../meta";
import type { NodeEventFeed } from "../events";
import type { ControlPlaneService } from "sepia-session-control";
import {
  BUNDLE_CONTENT_TYPE,
  fetchProjectBundle,
  importProjectBundle,
  parseRemoteUrl,
  postProjectBundle,
  projectBundleStream,
  transferSseResponse,
  type ImportExecutor,
  type RemoteEndpoint,
} from "../transfer";
import {
  errorMessage,
  isRecord,
  jsonResponse,
  readJsonBody,
  segmentsEqual,
  type EffectRunner,
  type RouteHandler,
} from "./shared";

export interface TransferRouteDeps {
  readonly run: EffectRunner;
  readonly plane: ControlPlaneService;
  readonly metaStore: MetaStore | undefined;
  readonly feed: NodeEventFeed;
  readonly node: NodeIdentity;
  readonly keepAliveMs: number;
  /**
   * Session-write executor shared with `/api/sessions/import` — `undefined`
   * when neither `importSession` nor `convert` is configured.
   */
  readonly executor: ImportExecutor | undefined;
  /** Test seam for the pull/push node-to-node fetch (defaults to fetch). */
  readonly fetchImpl: typeof fetch | undefined;
}

/**
 * Project transfer (docs/protocol.md "Project transfer").
 *
 * A project — its meta row plus every member session's full IR — moves
 * node-to-node as a streamed `application/x-ndjson` bundle. `export`
 * produces it, `import` consumes it, and `pull`/`push` are the verbs
 * clients call: they chain the two over HTTP (the node fetches/posts
 * the peer itself — no browser relay) and report progress over SSE.
 * Session writes reuse the same executor as /api/sessions/import, so
 * toolCalls, thinking, usage and checkpoints survive the hop intact.
 */
export const createTransferRoute =
  (deps: TransferRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    const { run, plane, metaStore, feed, node, keepAliveMs } = deps;
    const executor = deps.executor;
    const fetchImpl = deps.fetchImpl ?? fetch;

    /** `{source|target: {url, token?}}` validation shared by pull/push. */
    const remoteEndpoint = (body: unknown, key: "source" | "target"): RemoteEndpoint | Response => {
      const value = isRecord(body) ? body[key] : undefined;
      if (!isRecord(value)) {
        return jsonResponse({ error: `${key} must be an object {url, token?}` }, 400, cors);
      }
      const url = parseRemoteUrl(value.url);
      if (url instanceof Error) {
        return jsonResponse({ error: `${key}.url: ${url.message}` }, 400, cors);
      }
      if (value.token !== undefined && typeof value.token !== "string") {
        return jsonResponse({ error: `${key}.token must be a string` }, 400, cors);
      }
      return { url, ...(typeof value.token === "string" ? { token: value.token } : {}) };
    };

    if (
      method === "GET" &&
      segments[0] === "api" &&
      segments[1] === "projects" &&
      segments[3] === "export" &&
      segments.length === 4
    ) {
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      const projectId = decodeURIComponent(segments[2] ?? "");
      const project = meta.listProjects().find((p) => p.id === projectId);
      if (project === undefined) {
        return jsonResponse({ error: "Unknown project" }, 404, cors);
      }
      // Streamed line-by-line — a project of long sessions never buffers
      // whole in memory, and no request-size cap applies to a response.
      const body = projectBundleStream({ plane, run, meta, project, node });
      return new Response(body, {
        headers: {
          "content-type": BUNDLE_CONTENT_TYPE,
          "content-disposition": `attachment; filename="${project.id}.sepia.jsonl"`,
          ...cors,
        },
      });
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "projects", "import"])) {
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      if (executor === undefined) {
        return jsonResponse({ error: "Import is not configured on this server" }, 501, cors);
      }
      if (request.body === null) {
        return jsonResponse({ error: "Expected an NDJSON project bundle body" }, 400, cors);
      }
      // The body parses as a stream (readNdjsonLines), so bundle size never
      // hits the JSON body's in-memory ceiling — the 413 shape big
      // /api/sessions/import payloads risk.
      try {
        const result = await importProjectBundle(request.body, {
          executor,
          run,
          meta,
          feed,
          node,
        });
        return jsonResponse(result, 201, cors);
      } catch (error) {
        return jsonResponse({ error: errorMessage(error) }, 400, cors);
      }
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "projects", "pull"])) {
      const meta = metaStore;
      if (meta === undefined || executor === undefined) {
        return jsonResponse({ error: "Import is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const source = remoteEndpoint(body, "source");
      if (source instanceof Response) return source;
      const projectId = isRecord(body) ? body.project : undefined;
      if (typeof projectId !== "string" || projectId === "") {
        return jsonResponse({ error: "project is required" }, 400, cors);
      }
      // The receiving node pulls the bundle itself — the client's only role
      // was handing over the peer's address + credential.
      return transferSseResponse(
        async (emit) => {
          emit("start", { direction: "pull", source: source.url, project: projectId });
          const bundle = await fetchProjectBundle(source, projectId, fetchImpl, request.signal);
          if (bundle instanceof Error) {
            emit("error", { error: bundle.message });
            return;
          }
          const result = await importProjectBundle(bundle, {
            executor,
            run,
            meta,
            feed,
            node,
            onSession: (progress) => emit("session", progress),
          });
          emit("done", result);
        },
        request.signal,
        cors,
        keepAliveMs,
      );
    }

    if (
      method === "POST" &&
      segments[0] === "api" &&
      segments[1] === "projects" &&
      segments[3] === "push" &&
      segments.length === 4
    ) {
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Meta is not configured on this server" }, 501, cors);
      }
      const projectId = decodeURIComponent(segments[2] ?? "");
      const project = meta.listProjects().find((p) => p.id === projectId);
      if (project === undefined) {
        return jsonResponse({ error: "Unknown project" }, 404, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const target = remoteEndpoint(body, "target");
      if (target instanceof Response) return target;
      return transferSseResponse(
        async (emit) => {
          emit("start", { direction: "push", target: target.url, project: project.id });
          let index = 0;
          let total: number | undefined;
          // onLine taps the bundle as it generates — each session line is
          // both an upload step (the target streams the body) and a progress
          // frame here.
          const bundle = projectBundleStream({
            plane,
            run,
            meta,
            project,
            node,
            onLine: (record) => {
              if (record.type === "project" && typeof record.sessions === "number") {
                total = record.sessions;
              }
              if (record.type === "session") {
                index += 1;
                emit("session", {
                  index,
                  total,
                  id: record.id,
                  agent: record.agent,
                  title: record.title,
                });
              }
            },
          });
          const result = await postProjectBundle(target, bundle, fetchImpl, request.signal);
          if (result instanceof Error) {
            emit("error", { error: result.message });
            return;
          }
          emit("done", { target: target.url, ...result });
        },
        request.signal,
        cors,
        keepAliveMs,
      );
    }

    return undefined;
  };
