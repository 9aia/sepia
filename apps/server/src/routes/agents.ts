import type { ControlPlaneService } from "sepia-session-control";
import { jsonResponse, segmentsEqual, type RouteHandler } from "./shared";

export interface AgentRouteDeps {
  /** The standalone AG-UI agent endpoint handler (`POST /api/agent`). */
  readonly aguiAgent: (request: Request) => Promise<Response>;
}

/**
 * `POST /api/agent` — the standalone AG-UI agent endpoint kept for external
 * AG-UI clients (the web UI chats over `/api/sessions/:id/prompt` +
 * `/stream` instead). The inner handler sets its own headers; CORS rides on
 * top here.
 */
export const createAgentRoute =
  (deps: AgentRouteDeps): RouteHandler =>
  async ({ request, method, segments, cors }) => {
    if (method === "POST" && segmentsEqual(segments, ["api", "agent"])) {
      const response = await deps.aguiAgent(request);
      return new Response(response.body, {
        status: response.status,
        headers: { ...Object.fromEntries(response.headers), ...cors },
      });
    }
    return undefined;
  };

export interface AgentsRouteDeps {
  readonly plane: ControlPlaneService;
}

/** `GET /api/agents` — the agent runtime inventory. */
export const createAgentsRoute =
  (deps: AgentsRouteDeps): RouteHandler =>
  ({ method, segments, cors }) => {
    if (method === "GET" && segmentsEqual(segments, ["api", "agents"])) {
      return jsonResponse({ agents: deps.plane.listAgents() }, 200, cors);
    }
    return undefined;
  };
