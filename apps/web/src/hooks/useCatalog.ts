import { useMemo } from "react";
import { useStore } from "@tanstack/react-store";
import { buildCatalog, type Catalog, type CatalogNodeInput } from "../lib/catalog";
import { LOCAL_NODE_ID } from "../lib/format";
import { isPeerEnabled, nodeName } from "../lib/nodes";
import { settingsStore } from "../lib/settings";
import { useAgents } from "./query/useAgents";
import { useNodes, usePeerDescriptors } from "./query/useNodes";

/**
 * The federated agent/model catalog — Settings → Agents/Models/Nodes' data.
 *
 * Composes the existing node sources rather than fetching anything itself:
 * this machine's roster comes from `self.agents` (`useNodes`' populated
 * descriptor — `useSelfNode` should be mounted by the consumer so it
 * resolves), each enabled peer's from its `/api/node` descriptor
 * (`usePeerDescriptors`), and the merged `useAgents` roster stands in for
 * any node whose descriptor hasn't landed yet — the same fallback chain
 * the footer's Desktop menu uses. Parked nodes contribute nothing, exactly
 * like their fan-out legs.
 *
 * Model sets come from `settings.models` (see buildCatalog) — agents don't
 * advertise models on the wire, so configured prefs + the `auto` default
 * are the whole set.
 */
export const useCatalog = (): Catalog => {
  const settings = useStore(settingsStore);
  const { self, peers } = useNodes();
  const descriptors = usePeerDescriptors(peers);
  const { data: agents = [] } = useAgents();
  return useMemo(() => {
    const labels = new Map(agents.map((agent) => [agent.id, agent.label]));
    const fallbackIds = agents.map((agent) => agent.id);
    const roster = (ids: ReadonlyArray<string>) =>
      ids.map((id) => ({ id, label: labels.get(id) ?? id }));
    const input: CatalogNodeInput[] = [
      ...(settings.localNodeEnabled
        ? [
            {
              node: LOCAL_NODE_ID,
              label: nodeName(undefined),
              agents: roster(self?.agents ?? fallbackIds),
            },
          ]
        : []),
      ...peers.filter(isPeerEnabled).map((peer, index) => ({
        node: peer.id,
        label: peer.alias ?? peer.name,
        agents: roster(descriptors[index]?.agents ?? fallbackIds),
      })),
    ];
    return buildCatalog(input, settings);
    // descriptors' identity churns per render — rebuild when contents land.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings, self, peers, agents, ...descriptors]);
};
