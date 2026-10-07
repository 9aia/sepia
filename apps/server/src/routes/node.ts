import { hostname, userInfo } from "node:os";
import type { ControlPlaneService } from "sepia-session-control";
import { PROTOCOL_VERSION, type NodeIdentity } from "../node";
import { jsonResponse, segmentsEqual, type RouteHandler } from "./shared";

export interface NodeRouteDeps {
  readonly plane: ControlPlaneService;
  readonly node: NodeIdentity;
  /** Present when `POST /api/pair` is configured — reported in capabilities. */
  readonly pairing: boolean;
}

export const createNodeRoute =
  (deps: NodeRouteDeps): RouteHandler =>
  ({ method, segments, cors }) => {
    if (method === "GET" && segmentsEqual(segments, ["api", "user"])) {
      const info = userInfo();
      return jsonResponse(
        {
          user: {
            username: info.username,
            homedir: info.homedir,
            shell: info.shell,
            hostname: hostname(),
            platform: process.platform,
            arch: process.arch,
          },
        },
        200,
        cors,
      );
    }

    // Node metadata — the one endpoint every federated client hits first.
    if (method === "GET" && segmentsEqual(segments, ["api", "node"])) {
      return jsonResponse(
        {
          id: deps.node.id,
          name: deps.node.name,
          version: deps.node.version,
          protocol: PROTOCOL_VERSION,
          agents: deps.plane.listAgents().map((agent) => agent.id),
          capabilities: [
            "sessions",
            "projects",
            "push",
            "events",
            "export",
            "transfer",
            ...(deps.pairing ? ["pairing"] : []),
          ],
        },
        200,
        cors,
      );
    }

    return undefined;
  };
