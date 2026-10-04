import { useState, type FormEvent, type KeyboardEvent } from "react";
import { useStore } from "@tanstack/react-store";
import { Delete02Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  useAddNode,
  useNodes,
  useNodeStatuses,
  usePairNode,
  useRemoveNode,
  useSelfNode,
} from "../hooks/query/useNodes";
import { setPeerAlias } from "../lib/nodes";
import { setSettings, settingsStore } from "../lib/settings";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Switch } from "./ui/switch";

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
const blurOnEnter = (event: KeyboardEvent<HTMLInputElement>): void => {
  if (event.key === "Enter") event.currentTarget.blur();
};

export function NodesSection() {
  const { self, peers } = useNodes();
  const localName = useStore(settingsStore, (s) => s.localNodeName);
  const selfQuery = useSelfNode();
  const statuses = useNodeStatuses(peers);
  const addNode = useAddNode();
  const pairNode = usePairNode();
  const removeNode = useRemoveNode();
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [code, setCode] = useState("");
  const [mode, setMode] = useState<"code" | "token">("code");
  const [viaGateway, setViaGateway] = useState(false);

  const active = mode === "code" ? pairNode : addNode;
  const canSubmit =
    url.trim() !== "" && (mode === "token" || code.trim() !== "") && !active.isPending;

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (!canSubmit) return;
    const via = viaGateway ? ("gateway" as const) : ("direct" as const);
    const reset = () => {
      setUrl("");
      setToken("");
      setCode("");
    };
    if (mode === "code") {
      pairNode.mutate({ url, code, via }, { onSuccess: reset });
    } else {
      addNode.mutate({ url, token, via }, { onSuccess: reset });
    }
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
            <Input
              key={localName ?? self?.name ?? ""}
              className="h-7 w-44 text-sm font-medium"
              defaultValue={localName ?? ""}
              placeholder={self?.name ?? "This machine"}
              aria-label="Nickname for this machine"
              title="Nickname for this machine"
              onBlur={(e) =>
                setSettings({ localNodeName: e.currentTarget.value.trim() || null })
              }
              onKeyDown={blurOnEnter}
            />
            <span className="block truncate text-xs text-muted-foreground">
              {location.origin} — this machine
            </span>
          </div>
        </div>
        {peers.map((peer, index) => (
          <div key={peer.id} className="flex items-center gap-3 px-3 py-2.5">
            <StatusDot ok={statuses[index]} />
            <div className="min-w-0 flex-1">
              <Input
                key={`${peer.id}:${peer.alias ?? ""}`}
                className="h-7 w-44 text-sm font-medium"
                defaultValue={peer.alias ?? ""}
                placeholder={peer.name}
                aria-label={`Nickname for ${peer.name}`}
                title="Nickname — shown instead of the node's name"
                onBlur={(e) => setPeerAlias(peer.id, e.currentTarget.value)}
                onKeyDown={blurOnEnter}
              />
              <span className="block truncate text-xs text-muted-foreground">
                {peer.alias !== undefined ? `${peer.name} — ` : ""}
                {peer.url}
                {peer.via === "gateway" && " — via gateway"}
              </span>
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
        <div className="flex gap-1">
          <Button
            type="button"
            variant={mode === "code" ? "secondary" : "ghost"}
            size="xs"
            onClick={() => setMode("code")}
          >
            Pairing code
          </Button>
          <Button
            type="button"
            variant={mode === "token" ? "secondary" : "ghost"}
            size="xs"
            onClick={() => setMode("token")}
          >
            Token
          </Button>
        </div>
        <div className="grid gap-2 sm:grid-cols-2">
          <Input
            placeholder="https://hostname:8787"
            aria-label="Node URL"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
          {mode === "code" ? (
            <Input
              placeholder="Code from `sepia pair` (e.g. 7K2M-9PQX)"
              aria-label="Pairing code"
              value={code}
              onChange={(event) => setCode(event.target.value)}
            />
          ) : (
            <Input
              type="password"
              placeholder={
                viaGateway ? "Bearer token (stored on this node)" : "Bearer token (if required)"
              }
              aria-label="Node token"
              value={token}
              onChange={(event) => setToken(event.target.value)}
            />
          )}
        </div>
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <Switch
            checked={viaGateway}
            onCheckedChange={setViaGateway}
            aria-label="Route through this node"
          />
          Route through this node (gateway) — for peers the browser can't reach directly
        </label>
        {active.isError && (
          <p className="text-xs text-destructive">
            {active.error instanceof Error ? active.error.message : "Couldn't reach that node"}
          </p>
        )}
        <Button type="submit" variant="secondary" disabled={!canSubmit}>
          {active.isPending ? "Checking…" : mode === "code" ? "Pair node" : "Add node"}
        </Button>
      </form>
    </section>
  );
}
