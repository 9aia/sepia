import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CloudLoadingIcon, CloudOffIcon, FolderOpenIcon } from "@hugeicons/core-free-icons";
import { queryKeys } from "../../hooks/query/keys";
import { useNodesConnected } from "../../hooks/query/useNodes";
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
  // `useNodesConnected` carries the verdict — its peer probes run only while
  // this state is mounted (a populated list unmounts it and the status
  // queries go idle again… other mounts probing the same keys aside). The
  // 15s re-probe doubles as the recovery path: a node coming back flips
  // `connected` and the refetch below pulls its rows in.
  const connected = useNodesConnected();
  const queryClient = useQueryClient();

  // A node that answers again should contribute its sessions immediately —
  // nudge the merged list rather than waiting for a focus-driven refetch.
  useEffect(() => {
    if (connected !== "connected") return;
    void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
  }, [connected, queryClient]);

  if (connected === "connected") {
    return (
      <EmptyScreen
        className="p-6"
        icon={FolderOpenIcon}
        title="No sessions yet"
        description="Create your first session above."
      />
    );
  }
  if (connected === "checking") {
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
