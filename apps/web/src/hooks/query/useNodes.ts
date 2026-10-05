import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { getNode } from "../../lib/api";
import { LOCAL_NODE_ID } from "../../lib/format";
import { settingsStore } from "../../lib/settings";
import { toastError, toastSuccess } from "../../lib/toast";
import {
  addGatewayPeer,
  addPeer,
  isPeerEnabled,
  nodeName,
  nodesStore,
  pairGatewayPeer,
  pairPeer,
  peerTarget,
  refreshSelf,
  removePeerEntry,
  setLocalNodeEnabled,
  setPeerEnabled,
  updatePeerEntry,
  type PeerCredentialSpec,
  type PeerEntryUpdate,
  type PeerNode,
} from "../../lib/nodes";
import { queryKeys } from "./keys";

/**
 * The node registry as React state: `self` (filled by `useSelfNode`) plus
 * the persisted peer list. `peers.length > 0` is the multi-node switch every
 * federation affordance checks.
 */
export const useNodes = () => useStore(nodesStore);

export const useMultiNode = (): boolean => useStore(nodesStore, (s) => s.peers.some(isPeerEnabled));

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

/**
 * Per-peer reachability (green/red/grey dot). `undefined` means "not probed
 * yet" — the first fetch is in flight, or the peer is disabled (never
 * probed); `false` means the probe settled with a failure. A failed REFETCH
 * keeps the last answer (`isSuccess` survives with stale data) so the dot
 * doesn't flap.
 */
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
      enabled: isPeerEnabled(peer),
      retry: 1,
      refetchInterval: 15_000,
      staleTime: 10_000,
    })),
  }).map((result, index) => {
    const peer = peers[index];
    if (peer === undefined || !isPeerEnabled(peer)) return undefined;
    return result.isPending ? undefined : result.isSuccess;
  });

/**
 * "Is anything reachable" — the verdict that gates creation affordances and
 * the sidebar's sections. The local leg counts unless the user parked this
 * machine (`localNodeEnabled: false` — its fan-out leg never runs, so it
 * can't contribute) or its last probe failed (`selfStatus === "offline"`);
 * "unknown" reads optimistically connected because the merged fan-out's own
 * local leg is what settles it — treating boot as disconnected would flash
 * the no-nodes empty state on every load. Enabled peers are probed via
 * `useNodeStatuses`, but only while the local leg can't already answer:
 * a connected local makes the peer verdict irrelevant, and the
 * ClientBar/Settings mounts probe the same `node/status` keys anyway.
 *
 * - "connected" — some node can contribute; creation affordances may show.
 * - "checking" — nothing connected yet and ≥1 peer probe is still in flight.
 * - "disconnected" — local parked/offline and every enabled peer failed.
 */
export type NodesConnected = "connected" | "checking" | "disconnected";

export const useNodesConnected = (): NodesConnected => {
  const localEnabled = useStore(settingsStore, (s) => s.localNodeEnabled);
  const selfStatus = useStore(nodesStore, (s) => s.selfStatus);
  const peers = useStore(nodesStore, (s) => s.peers);
  const localConnected = localEnabled && selfStatus !== "offline";
  const statuses = useNodeStatuses(localConnected ? [] : peers.filter(isPeerEnabled));
  if (localConnected || statuses.some((status) => status === true)) return "connected";
  return statuses.some((status) => status === undefined) ? "checking" : "disconnected";
};

/**
 * Each peer's own /api/node descriptor, aligned with `peers` by index —
 * powers menus that list a peer's agents (e.g. "Resume on…"). Disabled peers
 * aren't probed; their slot stays `undefined`.
 */
export const usePeerDescriptors = (peers: ReadonlyArray<PeerNode>) =>
  useQueries({
    queries: peers.map((peer) => ({
      queryKey: [...queryKeys.node, "descriptor", peer.id],
      queryFn: () => getNode(peerTarget(peer)),
      enabled: isPeerEnabled(peer),
      retry: 1,
      staleTime: 60_000,
    })),
  }).map((result, index) => {
    const peer = peers[index];
    return peer === undefined || !isPeerEnabled(peer) ? undefined : result.data;
  });

/** How the client reaches the peer — directly, or through this node's gateway. */
export type PeerVia = "direct" | "gateway";

/**
 * Add + validate a peer, then refetch every merged list so it appears. A
 * direct add takes a `PeerCredentialSpec` (stored link or fresh secret —
 * the store files it under the node's name); a gateway add takes the raw
 * `token` for the managed registry.
 */
export const useAddNode = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      url,
      credential,
      token,
      via,
    }: {
      url: string;
      credential?: PeerCredentialSpec | null;
      token?: string;
      via?: PeerVia;
    }) => (via === "gateway" ? addGatewayPeer(url, token ?? "") : addPeer(url, credential ?? null)),
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

/**
 * Save the Settings → Nodes edit form: url/token/routing/SSH changes
 * (`PeerEntryUpdate`) — the alias is committed separately via `setPeerAlias`
 * in the caller since it needs no async work. A gateway peer's credential
 * and SSH tunnel ride `updateServer` inside `updatePeerEntry`, which also
 * moves the credential between the client and the managed registry when
 * `via` flips.
 */
export const useUpdateNode = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, update }: { id: string; update: PeerEntryUpdate }) =>
      updatePeerEntry(id, update),
    onSuccess: () => {
      void queryClient.invalidateQueries();
      toastSuccess("Node updated");
    },
    // The caller renders mutation.error inline — no toast here.
  });
};

/**
 * The per-row enable switch — `LOCAL_NODE_ID` parks the local node (a
 * settings pref), any other id parks a peer (registry). Toggling is
 * synchronous store work, but the merged lists/descriptors/event feeds must
 * re-sync — hence a mutation that just invalidates everything.
 */
export const useSetNodeEnabled = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) => {
      if (id === LOCAL_NODE_ID) setLocalNodeEnabled(enabled);
      else setPeerEnabled(id, enabled);
    },
    onSuccess: (_data, { enabled }) => {
      void queryClient.invalidateQueries();
      toastSuccess(enabled ? "Node enabled" : "Node disabled");
    },
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
