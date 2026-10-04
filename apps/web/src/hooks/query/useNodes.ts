import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { getNode } from "../../lib/api";
import { toastError, toastSuccess } from "../../lib/toast";
import {
  addGatewayPeer,
  addPeer,
  nodeName,
  nodesStore,
  pairGatewayPeer,
  pairPeer,
  peerTarget,
  refreshSelf,
  removePeerEntry,
  type PeerNode,
} from "../../lib/nodes";
import { queryKeys } from "./keys";

/**
 * The node registry as React state: `self` (filled by `useSelfNode`) plus
 * the persisted peer list. `peers.length > 0` is the multi-node switch every
 * federation affordance checks.
 */
export const useNodes = () => useStore(nodesStore);

export const useMultiNode = (): boolean => useStore(nodesStore, (s) => s.peers.length > 0);

/** The local node's /api/node descriptor — populates `nodesStore.self`. */
export const useSelfNode = () =>
  useQuery({
    queryKey: queryKeys.node,
    queryFn: refreshSelf,
    staleTime: 60_000,
    retry: 1,
  });

/** Display name for a row's `node` — reactive over the registry. */
export const useNodeLabel = (node: string | undefined): string =>
  useStore(nodesStore, () => nodeName(node));

/** Per-peer reachability for the Settings list (green/grey dot). */
export const useNodeStatuses = (
  peers: ReadonlyArray<PeerNode>,
): ReadonlyArray<boolean | undefined> =>
  useQueries({
    queries: peers.map((peer) => ({
      queryKey: [...queryKeys.node, "status", peer.id],
      queryFn: async () => {
        await getNode(peerTarget(peer));
        return true;
      },
      retry: 1,
      refetchInterval: 15_000,
      staleTime: 10_000,
    })),
  }).map((result) => (result.data === undefined ? undefined : result.isSuccess));

/**
 * Each peer's own /api/node descriptor, aligned with `peers` by index —
 * powers menus that list a peer's agents (e.g. "Resume on…").
 */
export const usePeerDescriptors = (peers: ReadonlyArray<PeerNode>) =>
  useQueries({
    queries: peers.map((peer) => ({
      queryKey: [...queryKeys.node, "descriptor", peer.id],
      queryFn: () => getNode(peerTarget(peer)),
      retry: 1,
      staleTime: 60_000,
    })),
  }).map((result) => result.data);

/** How the browser reaches the peer — directly, or through this node's gateway. */
export type PeerVia = "direct" | "gateway";

/** Add + validate a peer, then refetch every merged list so it appears. */
export const useAddNode = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ url, token, via }: { url: string; token: string; via?: PeerVia }) =>
      via === "gateway" ? addGatewayPeer(url, token) : addPeer(url, token),
    onSuccess: () => {
      void queryClient.invalidateQueries();
      toastSuccess("Node added");
    },
    // The caller renders mutation.error inline — no toast here.
  });
};

/**
 * "Pair with code": redeem the one-time code `sepia pair` printed on the
 * node (POST {url}/api/pair), then register the peer. The manual token path
 * (useAddNode) stays the fallback.
 */
export const usePairNode = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ url, code, via }: { url: string; code: string; via?: PeerVia }) =>
      via === "gateway" ? pairGatewayPeer(url, code) : pairPeer(url, code),
    onSuccess: () => {
      void queryClient.invalidateQueries();
      toastSuccess("Node paired");
    },
    // The caller renders mutation.error inline — no toast here.
  });
};

/** Drop a peer (and a gateway peer's managed credential); its rows vanish on the next refetch. */
export const useRemoveNode = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => removePeerEntry(id),
    onSuccess: () => {
      void queryClient.invalidateQueries();
      toastSuccess("Node removed");
    },
    onError: (error) => toastError("Couldn't remove the node", error),
  });
};
