import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeftIcon, Download01Icon, Upload01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { listProjects, type TransferEndpoint } from "../../lib/api";
import { isLocalNode, projectKey } from "../../lib/format";
import { isPeerEnabled, nodeName, peerTarget, type PeerNode } from "../../lib/nodes";
import { localEndpoint, peerEndpoint } from "../../lib/transfer";
import { useNodes } from "../../hooks/query/useNodes";
import { usePullProject, usePushProject } from "../../hooks/query/useProjects";
import type { Project } from "../../lib/types";
import { Button } from "../ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../ui/dialog";
import { ScrollArea } from "../ui/scroll-area";

/**
 * The push/pull dialogs behind a project row's transfer actions
 * (docs/protocol.md "Project transfer"). Both pick a peer from the node
 * registry; the resolved `{url, token}` goes to the transferring node, which
 * then talks to the peer itself — the browser never relays session data.
 */

interface TargetRow {
  readonly key: string;
  readonly label: string;
  /** Registered peer id — undefined for "this machine". */
  readonly node?: string;
  readonly endpoint: TransferEndpoint;
}

/** Push `project` to another node — pick the destination. */
export function PushProjectDialog({
  project,
  onClose,
}: {
  readonly project: Project;
  readonly onClose: () => void;
}) {
  const { peers } = useNodes();
  const push = usePushProject();
  // Destinations: every enabled peer that doesn't already own the project,
  // plus this machine when the project lives on a peer (that's a pull from
  // the project's node — the push dialog just phrases it as a target).
  const targets: ReadonlyArray<TargetRow> = [
    ...(isLocalNode(project.node)
      ? []
      : [{ key: "local", label: nodeName(undefined), node: undefined, endpoint: localEndpoint() }]),
    ...peers
      .filter(isPeerEnabled)
      .filter((peer) => peer.id !== project.node)
      .map((peer) => ({
        key: peer.id,
        label: peer.alias ?? peer.name,
        node: peer.id,
        endpoint: peerEndpoint(peer),
      })),
  ];
  const run = (target: TargetRow): void => {
    push.mutate({ project, endpoint: target.endpoint, label: target.label });
    onClose();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Push "{project.name}" to…</DialogTitle>
          <DialogDescription>
            The owning node bundles the project&apos;s sessions and sends them to the node you pick
            — the copy lands as a project there with full history.
          </DialogDescription>
        </DialogHeader>
        <ScrollArea className="max-h-64">
          <div className="flex flex-col gap-1.5 px-2">
            {targets.length === 0 && (
              <p className="px-1 py-2 text-sm text-muted-foreground">
                No other nodes are registered — add one in Settings → Nodes.
              </p>
            )}
            {targets.map((target) => (
              <button
                key={target.key}
                type="button"
                className="flex items-center gap-2 rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
                onClick={() => run(target)}
                disabled={push.isPending}
              >
                <HugeiconsIcon
                  icon={Upload01Icon}
                  strokeWidth={2}
                  className="size-4 shrink-0 text-muted-foreground"
                />
                <span className="min-w-0 flex-1 truncate font-medium">{target.label}</span>
              </button>
            ))}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}

/** Pull a project from a peer onto this machine — pick the peer, then the project. */
export function PullProjectDialog({ onClose }: { readonly onClose: () => void }) {
  const { peers } = useNodes();
  const pull = usePullProject();
  const [peer, setPeer] = useState<PeerNode | null>(null);
  const enabled = peers.filter(isPeerEnabled);
  const remote = useQuery({
    queryKey: ["remote-projects", peer?.id ?? ""],
    queryFn: () =>
      peer === null ? Promise.resolve({ projects: [] }) : listProjects(peerTarget(peer)),
    enabled: peer !== null,
  });
  const run = (remoteId: string): void => {
    if (peer === null) return;
    pull.mutate({
      endpoint: peerEndpoint(peer),
      remoteProjectId: remoteId,
      label: peer.alias ?? peer.name,
    });
    onClose();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          {peer !== null && (
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Back to nodes"
              onClick={() => setPeer(null)}
            >
              <HugeiconsIcon icon={ChevronLeftIcon} strokeWidth={2} />
            </Button>
          )}
          <DialogTitle>
            {peer === null ? "Pull a project from…" : `Pull from ${peer.alias ?? peer.name}`}
          </DialogTitle>
          <DialogDescription>
            {peer === null
              ? "Pick the node to fetch a project from — this machine downloads and imports it."
              : "Pick a project — it lands here as a copy with full session history."}
          </DialogDescription>
        </DialogHeader>
        <ScrollArea className="max-h-64">
          <div className="flex flex-col gap-1.5 px-2">
            {peer === null && enabled.length === 0 && (
              <p className="px-1 py-2 text-sm text-muted-foreground">
                No other nodes are registered — add one in Settings → Nodes.
              </p>
            )}
            {peer === null &&
              enabled.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className="flex items-center gap-2 rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
                  onClick={() => setPeer(p)}
                >
                  <HugeiconsIcon
                    icon={Download01Icon}
                    strokeWidth={2}
                    className="size-4 shrink-0 text-muted-foreground"
                  />
                  <span className="min-w-0 flex-1 truncate font-medium">{p.alias ?? p.name}</span>
                </button>
              ))}
            {peer !== null && remote.isPending && (
              <p className="px-1 py-2 text-sm text-muted-foreground">Loading projects…</p>
            )}
            {peer !== null && remote.isError && (
              <p className="px-1 py-2 text-sm text-destructive">
                Couldn&apos;t reach {peer.alias ?? peer.name}.
              </p>
            )}
            {peer !== null && remote.isSuccess && remote.data.projects.length === 0 && (
              <p className="px-1 py-2 text-sm text-muted-foreground">No projects there.</p>
            )}
            {peer !== null &&
              remote.data?.projects.map((project) => (
                <button
                  key={projectKey(project)}
                  type="button"
                  className="rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
                  onClick={() => run(project.id)}
                  disabled={pull.isPending}
                >
                  <span className="block truncate font-medium">{project.name}</span>
                  <span className="block truncate text-xs text-muted-foreground">{project.id}</span>
                </button>
              ))}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}
