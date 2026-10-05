import { useQuery } from "@tanstack/react-query";
import { listServers } from "../../lib/servers";
import { queryKeys } from "./keys";

/**
 * The managed-server registry — always queried against the local node.
 * There's no standalone Servers UI: entries exist to back `via: "gateway"`
 * peers (the entry IS the peer's server-side credential + SSH config), so
 * the only reader is the node edit dialog, which seeds its SSH fields from
 * the peer's entry. Mutations go through the node ops (`updatePeerEntry`
 * et al. in lib/nodes.ts), not a hook here.
 */
export const useServers = () =>
  useQuery({ queryKey: queryKeys.servers, queryFn: listServers, retry: 1 });
