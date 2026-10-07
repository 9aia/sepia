import type { ServerStore } from "../servers";
import { handleGatewayRoute, handleServersRoute } from "../servers-routes";
import type { TunnelManager } from "../ssh";
import { jsonResponse, type RouteHandler } from "./shared";

export interface ServersRouteTableDeps {
  readonly servers: ServerStore | undefined;
  readonly tunnels: TunnelManager | undefined;
}

/**
 * `/api/servers/*` and `/api/gateway/*` — both delegate to the handlers in
 * ../servers-routes.ts (which own the CRUD, tunnel and proxy logic).
 *
 * Gateway mode (docs/protocol.md phase 3): `ANY /api/gateway/:peer/*`
 * proxies a managed-server registry entry with its stored credential
 * injected — the federated UI's route to peers the client can't reach
 * directly. Behind the same bearer check as every other /api route.
 */
export const createServersRoute =
  (deps: ServersRouteTableDeps): RouteHandler =>
  async ({ request, segments, cors }) => {
    if (segments[0] === "api" && segments[1] === "servers") {
      const store = deps.servers;
      const tunnels = deps.tunnels;
      if (store === undefined || tunnels === undefined) {
        return jsonResponse(
          { error: "Server management is not configured on this server" },
          501,
          cors,
        );
      }
      const handled = await handleServersRoute(request, segments.slice(2), {
        store,
        tunnels,
        cors,
      });
      return handled ?? jsonResponse({ error: "Not found" }, 404, cors);
    }

    if (segments[0] === "api" && segments[1] === "gateway") {
      const store = deps.servers;
      const tunnels = deps.tunnels;
      if (store === undefined || tunnels === undefined) {
        return jsonResponse({ error: "Gateway is not configured on this server" }, 501, cors);
      }
      const handled = await handleGatewayRoute(request, segments.slice(2), {
        store,
        tunnels,
        cors,
      });
      return handled ?? jsonResponse({ error: "Not found" }, 404, cors);
    }

    return undefined;
  };
