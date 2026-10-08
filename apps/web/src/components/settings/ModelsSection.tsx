import { useMemo } from "react";
import { setModelEnabled, type CatalogModel } from "../../lib/catalog";
import { parseCatalogKey } from "../../lib/format";
import { useCatalog } from "../../hooks/useCatalog";
import { useSelfNode } from "../../hooks/query/useNodes";
import { Switch } from "../ui/switch";
import { CatalogEmptyCard, CatalogRow, CountBadge, DisabledBadge } from "./CatalogRow";

/**
 * Flat catalog models grouped by id — a model id appears once per
 * (node, agent) offering, and the group is the "who carries this model"
 * view. First-seen order keeps the "auto" agent-default group first.
 */
const groupByModelId = (
  models: ReadonlyArray<CatalogModel>,
): ReadonlyArray<[CatalogModel, ...CatalogModel[]]> => {
  const map = new Map<string, [CatalogModel, ...CatalogModel[]]>();
  for (const model of models) {
    const list = map.get(model.id);
    if (list === undefined) map.set(model.id, [model]);
    else list.push(model);
  }
  return [...map.values()];
};

/**
 * One model id's row — count of agents carrying it, the agents+nodes
 * summary as the subline, and a bulk switch. The expandable lists every
 * (agent, node) offering with its own `node:agent:model` switch — the
 * per-catalogKey toggle.
 */
function ModelGroupRow({
  group,
  agentLabel,
  nodeLabel,
}: {
  readonly group: [CatalogModel, ...CatalogModel[]];
  readonly agentLabel: (id: string) => string;
  readonly nodeLabel: (node: string) => string;
}) {
  const head = group[0];
  const allEnabled = group.every((model) => model.enabled);
  const noneEnabled = group.every((model) => !model.enabled);
  const setAll = (enabled: boolean): void => {
    for (const model of group) {
      const parsed = parseCatalogKey(model.key);
      if (parsed !== null) {
        setModelEnabled(parsed.node, parsed.agent, parsed.model, enabled);
      }
    }
  };
  return (
    <CatalogRow
      label={head.label}
      title={head.auto ? "Agent default" : head.label}
      badges={
        <>
          <CountBadge count={head.agents.length} unit="agents" title="Agents carrying this model" />
          {noneEnabled && <DisabledBadge />}
        </>
      }
      subline={`${head.agents.map(agentLabel).join(" · ")} — ${head.nodes
        .map(nodeLabel)
        .join(" · ")}`}
      dimmed={noneEnabled}
      trailing={
        <Switch
          checked={allEnabled}
          onCheckedChange={setAll}
          aria-label={`${allEnabled ? "Disable" : "Enable"} model ${head.label}`}
          title={allEnabled ? "Disable everywhere" : "Enable everywhere"}
        />
      }
    >
      <div className="flex flex-col divide-y divide-border/40">
        {group.map((model) => {
          const parsed = parseCatalogKey(model.key);
          if (parsed === null) return null;
          return (
            <div key={model.key} className="flex items-center gap-3 py-1.5">
              <span
                className={`min-w-0 flex-1 truncate text-xs${model.enabled ? "" : " opacity-60"}`}
              >
                {agentLabel(parsed.agent)}
                <span className="text-muted-foreground">{` on ${nodeLabel(parsed.node)}`}</span>
              </span>
              <Switch
                checked={model.enabled}
                onCheckedChange={(value) =>
                  setModelEnabled(parsed.node, parsed.agent, parsed.model, value)
                }
                aria-label={`${model.enabled ? "Disable" : "Enable"} ${head.label} for ${agentLabel(
                  parsed.agent,
                )} on ${nodeLabel(parsed.node)}`}
              />
            </div>
          );
        })}
      </div>
      {head.auto && (
        <p className="pt-2 text-xs text-muted-foreground">
          Whatever each agent runs without a model override. Parking it hides the option in pickers
          — a spawn without a model arg still hits the agent&apos;s own default.
        </p>
      )}
    </CatalogRow>
  );
}

/**
 * Settings → Models: the federated model roster — one row per model id
 * across the catalog. Agents don't advertise models on the wire, so the
 * set comes from each agent's configured model pref (edited under Settings
 * → Agents) plus the agent-default entry; parking a model keeps it out of
 * pickers and spawn-time args on that node+agent.
 */
export function ModelsSection({ scrollTo }: { readonly scrollTo: (id: string) => void }) {
  // Resolves self.agents — the catalog's local roster source.
  useSelfNode();
  const catalog = useCatalog();
  const groups = useMemo(() => groupByModelId(catalog.models), [catalog.models]);
  const agentLabel = (id: string): string =>
    catalog.agents.find((agent) => agent.id === id)?.label ?? id;
  const nodeLabel = (node: string): string =>
    catalog.nodes.find((entry) => entry.node === node)?.label ?? node;

  return (
    <section data-spy="models" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Models</h3>
      <p className="text-xs text-muted-foreground">
        Every model the connected nodes can run — the roster comes from each agent&apos;s configured
        model pref (Settings → Agents). Parking one keeps it out of pickers and spawn-time model
        args on that node+agent.
      </p>
      {groups.length === 0 ? (
        <CatalogEmptyCard
          title="No models"
          body="Models come from connected nodes — connect one to see what it can run."
          onConnect={() => scrollTo("nodes")}
        />
      ) : (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          {groups.map((group) => (
            <ModelGroupRow
              key={group[0].id}
              group={group}
              agentLabel={agentLabel}
              nodeLabel={nodeLabel}
            />
          ))}
        </div>
      )}
    </section>
  );
}
