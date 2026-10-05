import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Tick02Icon, Copy01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { getNode } from "../../lib/api";
import { isThisMachine, localNodeAddress, nodeName, nodeTarget, nodesStore } from "../../lib/nodes";
import { LOCAL_NODE_ID } from "../../lib/format";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../ui/dialog";
import { useStore } from "@tanstack/react-store";

function Row({
  label,
  value,
  copyValue,
  mono,
}: {
  readonly label: string;
  readonly value: string;
  readonly copyValue?: string;
  readonly mono?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const copy = (): void => {
    if (copyValue === undefined) return;
    void navigator.clipboard?.writeText(copyValue).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      },
      () => undefined,
    );
  };
  return (
    <div className="flex items-center justify-between gap-4 py-2 text-sm">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="flex min-w-0 items-center justify-end gap-1">
        <span className={`truncate ${mono === true ? "font-mono text-xs" : ""}`} title={value}>
          {value}
        </span>
        {copyValue !== undefined && (
          <Button
            variant="ghost"
            size="icon-xs"
            className="size-5 shrink-0 text-muted-foreground"
            aria-label={`Copy ${label}`}
            title={`Copy ${label}`}
            onClick={copy}
          >
            <HugeiconsIcon icon={copied ? Tick02Icon : Copy01Icon} strokeWidth={2} />
          </Button>
        )}
      </span>
    </div>
  );
}

/**
 * Node/server details — opened from the session details "Server" row.
 * Shows the node's descriptor (id, version, agents, capabilities) plus
 * its address, reachability and registration details (nickname, gateway
 * hop, disabled state). Local resolves through `localNodeAddress()` —
 * the override or the serving origin.
 */
export function NodeDetailsDialog({
  node,
  open,
  onOpenChange,
}: {
  readonly node: string | undefined;
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { peers, selfStatus } = useStore(nodesStore);
  const isLocal = node === undefined || node === LOCAL_NODE_ID;
  const peer = isLocal ? undefined : peers.find((p) => p.id === node);
  const address = isLocal ? localNodeAddress() : (peer?.url ?? "");
  const descriptor = useQuery({
    queryKey: ["node", "details", node ?? LOCAL_NODE_ID],
    queryFn: () => getNode(nodeTarget(node)),
    enabled: open,
    staleTime: 15_000,
  });
  const data = descriptor.data;
  const status =
    peer?.enabled === false
      ? "disabled"
      : descriptor.isSuccess
        ? "reachable"
        : descriptor.isError
          ? "unreachable"
          : isLocal && selfStatus === "offline"
            ? "unreachable"
            : "checking";
  const name = nodeName(node);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <span className="truncate">{name}</span>
            {address !== "" && isThisMachine(address) && (
              <Badge variant="outline" className="h-4 shrink-0 px-1.5 text-[10px]">
                this machine
              </Badge>
            )}
          </DialogTitle>
          <DialogDescription>
            {isLocal ? "The node this client treats as local." : "A registered peer node."}
          </DialogDescription>
        </DialogHeader>
        <div className="divide-y divide-border/50">
          <Row
            label="Status"
            value={
              status === "reachable"
                ? "reachable"
                : status === "unreachable"
                  ? "unreachable"
                  : status === "disabled"
                    ? "disabled"
                    : "checking…"
            }
          />
          {data !== undefined && <Row label="ID" value={data.id} copyValue={data.id} mono />}
          {address !== "" && <Row label="Address" value={address} copyValue={address} mono />}
          {peer?.alias !== undefined && <Row label="Nickname" value={peer.alias} />}
          {peer?.via === "gateway" && <Row label="Routing" value="via this node's gateway" />}
          {data !== undefined && (
            <Row label="Version" value={`sepia ${data.version} · protocol ${data.protocol}`} />
          )}
          {data !== undefined && (
            <Row
              label="Agents"
              value={data.agents.length === 0 ? "none" : data.agents.join(", ")}
            />
          )}
          {data !== undefined && (
            <Row
              label="Capabilities"
              value={data.capabilities.length === 0 ? "baseline" : data.capabilities.join(" · ")}
            />
          )}
        </div>
        {descriptor.isError && (
          <p className="text-xs text-muted-foreground">
            Couldn&apos;t reach the node — showing what the client has registered.
          </p>
        )}
      </DialogContent>
    </Dialog>
  );
}
