import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { CloudLoadingIcon, CloudOffIcon, FolderOpenIcon } from "@hugeicons/core-free-icons";
import { queryKeys } from "../../hooks/query/keys";
import { useNodeStatuses } from "../../hooks/query/useNodes";
import { isPeerEnabled, nodesStore } from "../../lib/nodes";
import { settingsStore } from "../../lib/settings";
import { setSettingsOpen } from "../../lib/store";
import { EmptyScreen } from "../EmptyScreen";
import { Button } from "../ui/button";

/**
 * The empty session list. The client is the container — it loaded, so it
 * works; nodes contribute sessions. Zero reachable nodes is the default
 * "nothing connected yet" state with a Connect affordance, not an outage
 * banner: an unreachable origin node degrades exactly like a dead peer.
 * With any node up, an empty list is a plain "No sessions yet".
 */
export function ListEmptyState() {
  const selfOffline = useStore(nodesStore, (s) => s.selfStatus === "offline");
  const peers = useStore(nodesStore, (s) => s.peers);
  // A parked local node contributes nothing — it doesn't count as connected.
  const localEnabled = useStore(settingsStore, (s) => s.localNodeEnabled);
  const enabledPeers = peers.filter(isPeerEnabled);
  const queryClient = useQueryClient();
  // Peer reachability probes run only while this state is mounted — a
  // populated list unmounts it and the status queries go idle again. The
  // 15s re-probe doubles as the recovery path: a node coming back flips
  // `reachable` and the refetch below pulls its rows in.
  const statuses = useNodeStatuses(enabledPeers);
  const reachable = (localEnabled && !selfOffline) || statuses.some((status) => status === true);
  const probing = !reachable && statuses.some((status) => status === undefined);

  // A node that answers again should contribute its sessions immediately —
  // nudge the merged list rather than waiting for a focus-driven refetch.
  useEffect(() => {
    if (!reachable) return;
    void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
  }, [reachable, queryClient]);

  if (reachable) {
    return (
      <EmptyScreen
        className="p-6"
        icon={FolderOpenIcon}
        title="No sessions yet"
        description="Create your first session above."
      />
    );
  }
  if (probing) {
    return (
      <EmptyScreen
        className="p-6"
        icon={CloudLoadingIcon}
        title="Checking nodes"
        description="Reaching each registered node — sessions appear when one answers."
      />
    );
  }
  return (
    <EmptyScreen
      className="p-6"
      icon={CloudOffIcon}
      title="No nodes connected"
      description="Sessions come from nodes this client can reach — connect one to see them here."
    >
      <Button variant="secondary" onClick={() => setSettingsOpen(true, "nodes")}>
        Connect a node
      </Button>
    </EmptyScreen>
  );
}
