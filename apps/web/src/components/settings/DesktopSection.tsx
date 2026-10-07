import { useStore } from "@tanstack/react-store";
import { LOCAL_NODE_ID } from "../../lib/format";
import { isPeerEnabled, type PeerNode } from "../../lib/nodes";
import { setDesktop, settingsStore } from "../../lib/settings";
import { useAgents } from "../../hooks/query/useAgents";
import {
  useNodeLabel,
  useNodes,
  useNodesConnected,
  usePeerDescriptors,
  useSelfNode,
} from "../../hooks/query/useNodes";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "../ui/select";
import { CatalogEmptyCard } from "./CatalogRow";

/** Display label for a node option — nicknames win, then reported names. */
const peerLabel = (peer: PeerNode | undefined, id: string): string =>
  peer === undefined ? id : (peer.alias ?? peer.name);

/**
 * "Desktop" — the client's current working environment: which node,
 * agent, model and working directory "New session" aims at. The four picks
 * write `settings.desktop` directly — this is the same state the sidebar
 * footer's focus menu sets — and every field falls back when unset
 * (node → this machine, agent → the node's pick, model → the agent's
 * configured pref, dir → the node's most recent session's). Mounted only
 * while the dialog is open, so the self/peer roster probes don't run in
 * the background. With nothing reachable the fields hide behind the same
 * "connect a node" card the catalog sections use — a stale "local" picker
 * would lie.
 */
export function DesktopSection({ scrollTo }: { readonly scrollTo: (id: string) => void }) {
  const desktop = useStore(settingsStore, (state) => state.desktop);
  const { data: agents = [] } = useAgents();
  const { self, peers } = useNodes();
  useSelfNode();
  const connected = useNodesConnected();
  const descriptors = usePeerDescriptors(peers);
  const enabledPeers = peers.filter(isPeerEnabled);
  // A peer disabled or removed while the dialog is open leaves the stored
  // node dangling — the select just shows its id; the agent/dir picks stay
  // scoped to it and re-activate if the peer returns.
  const node = desktop.node ?? LOCAL_NODE_ID;
  const localLabel = useNodeLabel(undefined);
  const nodeLabel = (id: string): string =>
    id === LOCAL_NODE_ID
      ? localLabel
      : peerLabel(
          peers.find((p) => p.id === id),
          id,
        );
  // The agent roster of the desktop's node — the local descriptor knows
  // its own, a peer's comes from its /api/node probe; until either lands
  // the merged roster stands in.
  const fallbackIds = agents.map((agent) => agent.id);
  const peerIndex = peers.findIndex((peer) => peer.id === node);
  const rosterIds =
    node === LOCAL_NODE_ID
      ? (self?.agents ?? fallbackIds)
      : (descriptors[peerIndex]?.agents ?? fallbackIds);
  const agentLabel = (id: string): string => agents.find((agent) => agent.id === id)?.label ?? id;
  const agent = desktop.agent;
  // A stored id missing from the node's roster still shows — clearing it
  // is the user's call, not the select's.
  const agentOptions =
    agent !== null && !rosterIds.includes(agent) ? [...rosterIds, agent] : rosterIds;
  return (
    <section data-spy="desktop" className="flex scroll-mt-2 flex-col gap-4">
      <h3 className="text-sm font-medium">Desktop</h3>
      <p className="text-xs text-muted-foreground">
        The environment new sessions spawn in — the footer&apos;s focus menu sets the same picks.
      </p>
      {connected === "disconnected" ? (
        <CatalogEmptyCard
          title="No nodes connected"
          body="The desktop's picks apply on whatever node runs the session — connect one to see its agents and directories."
          onConnect={() => scrollTo("nodes")}
        />
      ) : (
        <>
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="desktop-node">
              Node
            </label>
            <Select
              value={node}
              onValueChange={(value) =>
                // The scoped picks don't port across nodes — a node change
                // resets agent/model/dir and lets the new node's fallbacks run.
                setDesktop({
                  node: value === undefined || value === LOCAL_NODE_ID ? null : value,
                  agent: null,
                  model: null,
                  cwd: null,
                })
              }
            >
              <SelectTrigger id="desktop-node" aria-label="Desktop node" className="w-full">
                {nodeLabel(node)}
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={LOCAL_NODE_ID}>{localLabel}</SelectItem>
                {enabledPeers.map((peer) => (
                  <SelectItem key={peer.id} value={peer.id}>
                    {peerLabel(peer, peer.id)}
                  </SelectItem>
                ))}
                {/* A stored node that isn't a registered/enabled peer still
                    shows so the pick isn't silently dropped. */}
                {node !== LOCAL_NODE_ID && !enabledPeers.some((peer) => peer.id === node) && (
                  <SelectItem value={node}>{node}</SelectItem>
                )}
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="desktop-agent">
              Agent
            </label>
            <Select
              value={agent ?? "__server__"}
              onValueChange={(value) =>
                setDesktop({
                  agent: value === "__server__" ? null : value,
                  // A model picked for the old agent doesn't apply to the new.
                  model: null,
                })
              }
            >
              <SelectTrigger id="desktop-agent" aria-label="Desktop agent" className="w-full">
                {agent === null ? "Node default" : agentLabel(agent)}
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__server__">Node default</SelectItem>
                {agentOptions.map((id) => (
                  <SelectItem key={id} value={id}>
                    {agentLabel(id)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Preselected when creating a session on {nodeLabel(node)} — unset means the node picks.
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="desktop-model">
              Model
            </label>
            <Input
              id="desktop-model"
              placeholder="Agent default"
              value={desktop.model ?? ""}
              onChange={(event) =>
                setDesktop({
                  model: event.target.value.trim() === "" ? null : event.target.value,
                })
              }
            />
            <p className="text-xs text-muted-foreground">
              Spawn-time model override; empty uses the agent&apos;s configured pref (Settings →
              Agents) or its own default.
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="desktop-cwd">
              Working directory
            </label>
            <Input
              id="desktop-cwd"
              placeholder="Most recent session's directory"
              value={desktop.cwd ?? ""}
              onChange={(event) =>
                setDesktop({
                  cwd: event.target.value.trim() === "" ? null : event.target.value,
                })
              }
            />
            <p className="text-xs text-muted-foreground">
              A path on {nodeLabel(node)}; empty uses the most recent session&apos;s directory
              there.
            </p>
          </div>
        </>
      )}
    </section>
  );
}
