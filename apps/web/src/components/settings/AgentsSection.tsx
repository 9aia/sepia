import { useMemo } from "react";
import { useStore } from "@tanstack/react-store";
import { setAgentEnabled, type CatalogAgent } from "../../lib/catalog";
import { setSettings, settingsStore, type AgentModelPref } from "../../lib/settings";
import { useCatalog } from "../../hooks/useCatalog";
import { useSelfNode } from "../../hooks/query/useNodes";
import { Input } from "../ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "../ui/select";
import { Switch } from "../ui/switch";
import { CatalogEmptyCard, CatalogRow, CountBadge, DisabledBadge, ModelList } from "./CatalogRow";

/**
 * Flat catalog agents grouped by id — an agent id appears once per node
 * offering it, and the group is the "which nodes carry it" view.
 */
const groupByAgentId = (
  agents: ReadonlyArray<CatalogAgent>,
): ReadonlyArray<[CatalogAgent, ...CatalogAgent[]]> => {
  const map = new Map<string, [CatalogAgent, ...CatalogAgent[]]>();
  for (const agent of agents) {
    const list = map.get(agent.id);
    if (list === undefined) map.set(agent.id, [agent]);
    else list.push(agent);
  }
  return [...map.values()];
};

const EMPTY_PREF: AgentModelPref = { model: "", fallbacks: "", mode: "auto" };

/**
 * The per-agent spawn-time model pref — `settings.models[agentId]`, global
 * per agent id so it applies on every node. Moved here from the old Models
 * section: it lives inside the agent row's detail, next to the model set
 * it produces.
 */
function AgentModelPrefEditor({
  agentId,
  label,
}: {
  readonly agentId: string;
  readonly label: string;
}) {
  const pref = useStore(settingsStore, (s) => s.models[agentId]) ?? EMPTY_PREF;
  const update = (patch: Partial<AgentModelPref>): void =>
    setSettings({
      models: { ...settingsStore.state.models, [agentId]: { ...pref, ...patch } },
    });
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border/60 p-2.5">
      <span className="text-xs text-muted-foreground">
        Spawn-time model pref — applies on every node. Auto mode also sends the fallback list
        (devin&apos;s refusal-fallback).
      </span>
      <div className="grid gap-2 sm:grid-cols-2">
        <Input
          placeholder="Model (empty = agent default)"
          aria-label={`${label} model`}
          value={pref.model}
          onChange={(event) => update({ model: event.target.value })}
        />
        <Select
          value={pref.mode}
          onValueChange={(v) => update({ mode: v as AgentModelPref["mode"] })}
        >
          <SelectTrigger aria-label={`${label} fallback mode`}>
            {pref.mode === "auto" ? "Auto fallback" : "Manual"}
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto fallback</SelectItem>
            <SelectItem value="manual">Manual</SelectItem>
          </SelectContent>
        </Select>
      </div>
      {pref.mode === "auto" && (
        <Input
          placeholder="Fallback models, comma-separated"
          aria-label={`${label} fallback models`}
          value={pref.fallbacks}
          onChange={(event) => update({ fallbacks: event.target.value })}
        />
      )}
    </div>
  );
}

/**
 * One agent id's row — count of models it offers, the nodes carrying it as
 * the subline, and a bulk switch. The expandable lists each node's
 * offering (its model set plus the per-`node:agent` switch) and the
 * agent's spawn-time model pref.
 */
function AgentGroupRow({
  group,
  nodeLabel,
}: {
  readonly group: [CatalogAgent, ...CatalogAgent[]];
  readonly nodeLabel: (node: string) => string;
}) {
  const head = group[0];
  const allEnabled = group.every((agent) => agent.enabled);
  const noneEnabled = group.every((agent) => !agent.enabled);
  const setAll = (enabled: boolean): void => {
    for (const agent of group) setAgentEnabled(agent.node, agent.id, enabled);
  };
  return (
    <CatalogRow
      label={head.label}
      title={head.label}
      badges={
        <>
          <CountBadge count={head.models.length} title="Models on this agent" />
          {noneEnabled && <DisabledBadge />}
        </>
      }
      subline={`on ${group.map((agent) => nodeLabel(agent.node)).join(" · ")}`}
      dimmed={noneEnabled}
      trailing={
        <Switch
          checked={allEnabled}
          onCheckedChange={setAll}
          aria-label={`${allEnabled ? "Disable" : "Enable"} agent ${head.label}`}
          title={allEnabled ? "Disable everywhere" : "Enable everywhere"}
        />
      }
    >
      <div className="flex flex-col gap-2.5">
        <div className="flex flex-col divide-y divide-border/40">
          {group.map((agent) => (
            <div key={agent.key} className="flex items-center gap-3 py-1.5">
              <div className={`min-w-0 flex-1${agent.enabled ? "" : " opacity-60"}`}>
                <span className="block text-xs font-medium">on {nodeLabel(agent.node)}</span>
                <ModelList models={agent.models} />
              </div>
              <Switch
                checked={agent.enabled}
                onCheckedChange={(value) => setAgentEnabled(agent.node, agent.id, value)}
                aria-label={`${agent.enabled ? "Disable" : "Enable"} agent ${agent.label} on ${nodeLabel(agent.node)}`}
              />
            </div>
          ))}
        </div>
        <AgentModelPrefEditor agentId={head.id} label={head.label} />
      </div>
    </CatalogRow>
  );
}

/**
 * Settings → Agents: the federated agent roster — one row per agent id
 * across the catalog, with which nodes carry it and its model set in the
 * detail. Parking an agent hides it from the footer picks and treats a
 * stored pick naming it as unset; the roster still lists it so the toggle
 * can bring it back.
 */
export function AgentsSection({ scrollTo }: { readonly scrollTo: (id: string) => void }) {
  // Resolves self.agents — the catalog's local roster source.
  useSelfNode();
  const catalog = useCatalog();
  const groups = useMemo(() => groupByAgentId(catalog.agents), [catalog.agents]);
  const nodeLabel = (node: string): string =>
    catalog.nodes.find((entry) => entry.node === node)?.label ?? node;

  return (
    <section data-spy="agents" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Agents</h3>
      <p className="text-xs text-muted-foreground">
        The agents connected nodes offer — parking one hides it from pickers and new-session
        resolution. Expand a row for its per-node model set and its spawn-time model pref.
      </p>
      {groups.length === 0 ? (
        <CatalogEmptyCard
          title="No agents"
          body="Agents come from connected nodes — connect one to see what it can run."
          onConnect={() => scrollTo("nodes")}
        />
      ) : (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          {groups.map((group) => (
            <AgentGroupRow key={group[0].id} group={group} nodeLabel={nodeLabel} />
          ))}
        </div>
      )}
    </section>
  );
}
