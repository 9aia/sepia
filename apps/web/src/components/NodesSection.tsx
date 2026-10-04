import { useState, type FormEvent } from "react";
import { Delete02Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  useAddNode,
  useNodes,
  useNodeStatuses,
  useRemoveNode,
  useSelfNode,
} from "../hooks/query/useNodes";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

/** Reachability dot — undefined while the first probe is in flight. */
function StatusDot({ ok }: { readonly ok: boolean | undefined }) {
  const color =
    ok === undefined ? "bg-muted-foreground/40" : ok ? "bg-emerald-500" : "bg-destructive";
  return (
    <span
      className={`inline-block size-2 shrink-0 rounded-full ${color}`}
      title={ok === undefined ? "Checking…" : ok ? "Reachable" : "Unreachable"}
    />
  );
}

/**
 * Settings → Nodes: the peer registry behind the federated lists. Adding a
 * node validates it through `GET /api/node` (a wrong URL or token fails
 * here, before it can poison the merged lists).
 */
export function NodesSection() {
  const { self, peers } = useNodes();
  const selfQuery = useSelfNode();
  const statuses = useNodeStatuses(peers);
  const addNode = useAddNode();
  const removeNode = useRemoveNode();
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (url.trim() === "" || addNode.isPending) return;
    addNode.mutate(
      { url, token },
      {
        onSuccess: () => {
          setUrl("");
          setToken("");
        },
      },
    );
  };

  return (
    <section data-spy="nodes" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Nodes</h3>
      <p className="text-xs text-muted-foreground">
        Other machines running <code>sepia serve</code>. Their sessions, projects and chat merge
        into this UI — actions go to the machine that owns each session.
      </p>
      <div className="divide-y divide-border/50 rounded-lg border border-border">
        <div className="flex items-center gap-3 px-3 py-2.5">
          <StatusDot ok={selfQuery.isSuccess || self !== null} />
          <div className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium">
              {self?.name ?? "This machine"}
            </span>
            <span className="block truncate text-xs text-muted-foreground">
              {location.origin} — this machine
            </span>
          </div>
        </div>
        {peers.map((peer, index) => (
          <div key={peer.id} className="flex items-center gap-3 px-3 py-2.5">
            <StatusDot ok={statuses[index]} />
            <div className="min-w-0 flex-1">
              <span className="block truncate text-sm font-medium">{peer.name}</span>
              <span className="block truncate text-xs text-muted-foreground">{peer.url}</span>
            </div>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={`Remove node ${peer.name}`}
              title="Remove node"
              onClick={() => removeNode.mutate(peer.id)}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
            </Button>
          </div>
        ))}
      </div>
      <form onSubmit={submit} className="flex flex-col gap-2">
        <div className="grid gap-2 sm:grid-cols-2">
          <Input
            placeholder="https://hostname:8787"
            aria-label="Node URL"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
          <Input
            type="password"
            placeholder="Bearer token (if required)"
            aria-label="Node token"
            value={token}
            onChange={(event) => setToken(event.target.value)}
          />
        </div>
        {addNode.isError && (
          <p className="text-xs text-destructive">
            {addNode.error instanceof Error ? addNode.error.message : "Couldn't reach that node"}
          </p>
        )}
        <Button type="submit" variant="secondary" disabled={url.trim() === "" || addNode.isPending}>
          {addNode.isPending ? "Checking…" : "Add node"}
        </Button>
      </form>
    </section>
  );
}
