import type { ReactNode } from "react";
import { useStore } from "@tanstack/react-store";
import { useHasKeyboard } from "../../lib/keyboard";
import {
  ArrowUp01Icon,
  BotIcon,
  BrainIcon,
  ComputerIcon,
  FolderIcon,
  KeyboardIcon,
  ServerIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useAppHotkey } from "../../lib/keybinds";
import { sepiaStore, setSettingsOpen } from "../../lib/store";
import { settingsStore } from "../../lib/settings";
import { useClient } from "../../lib/client";
import { clearFocus, setFocus, useFocus } from "../../lib/focus";
import { isAgentEnabled, isModelEnabled } from "../../lib/catalog";
import { isPeerEnabled, nodeName } from "../../lib/nodes";
import { LOCAL_NODE_ID, nodeKey } from "../../lib/format";
import { useAgents } from "../../hooks/query/useAgents";
import {
  useNodes,
  useNodesConnected,
  useNodeStatuses,
  usePeerDescriptors,
} from "../../hooks/query/useNodes";
import { useSessions } from "../../hooks/query/useSessions";
import { SettingsDialog } from "../SettingsDialog";
import { Button } from "../ui/button";
import { Skeleton } from "../ui/skeleton";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

/** One targetable node — this node first, then every enabled peer. */
interface DesktopEntry {
  /** nodeKey — "local" or the peer's registered id. */
  readonly node: string;
  readonly label: string;
  readonly agents: ReadonlyArray<string>;
  /** false = probed and failed; undefined = not probed yet. */
  readonly reachable: boolean | undefined;
}

/** The check mark shared by the desktop menu's current picks. */
const Check = () => <span className="ml-auto text-xs text-primary">✓</span>;

/** Last two path segments (`9aia/sepia`) — the menu's compact dir label. */
const dirLabel = (path: string): string => {
  const parts = path
    .replace(/\/+$/, "")
    .split("/")
    .filter((p) => p !== "");
  return parts.length === 0 ? path : parts.slice(-2).join("/");
};

/** Trigger row for the Model/Directory submenus — label left, current pick right. */
const SubValue = ({ children }: { children: ReactNode }) => (
  <span className="truncate text-xs font-normal text-muted-foreground">{children}</span>
);

/**
 * The sidebar footer: this client (browser/device identity — label +
 * keypair, managed in Settings → Client) plus the *desktop* summary —
 * the node+agent+model "New session" aims at, the same
 * `settings.desktop` Settings → Desktop edits. The dot tracks the
 * desktop node's reachability (selfStatus for this node, the peer
 * probe otherwise). The menu edits the desktop: a node submenu per
 * enabled node picks node+agent, while first-class Agent, Model and
 * Directory submenus cover the focused node's remaining fields. A
 * desktop node that goes offline or gets disabled dims and reads
 * "unreachable" until the user repicks; with no node reachable at all
 * the summary reads "No nodes connected".
 */
export function ClientBar() {
  const hasKeyboard = useHasKeyboard();
  const client = useClient();
  const desktop = useFocus();
  const modelPrefs = useStore(settingsStore, (state) => state.models);
  // Parked agents/models stay out of the pick menus — "not offered on that
  // node". A stored-but-parked pick still displays in the summary (below);
  // resolveCreateTarget/modelArgsFor skip applying it at spawn.
  const disabledAgents = useStore(settingsStore, (state) => state.disabledAgents);
  const disabledModels = useStore(settingsStore, (state) => state.disabledModels);
  const { self, selfStatus, peers } = useNodes();
  const nodesConnected = useNodesConnected();
  const statuses = useNodeStatuses(peers);
  const descriptors = usePeerDescriptors(peers);
  const { data: agents = [] } = useAgents();
  const { data: sessions = [] } = useSessions();
  const settingsOpen = useStore(sepiaStore, (state) => state.settingsOpen);
  useAppHotkey("app.settings", () => setSettingsOpen(!settingsOpen));
  const openSettings = (open: boolean): void => setSettingsOpen(open);

  // The merged roster stands in for a node whose descriptor hasn't landed.
  const fallbackIds = agents.map((agent) => agent.id);
  const flagStore = { disabledAgents, disabledModels };
  const entries: DesktopEntry[] = [
    {
      node: LOCAL_NODE_ID,
      label: nodeName(undefined),
      agents: (self?.agents ?? fallbackIds).filter((id) =>
        isAgentEnabled(flagStore, LOCAL_NODE_ID, id),
      ),
      reachable: selfStatus !== "offline",
    },
    ...peers.flatMap((peer, index) =>
      isPeerEnabled(peer)
        ? [
            {
              node: peer.id,
              label: peer.alias ?? peer.name,
              agents: (descriptors[index]?.agents ?? fallbackIds).filter((id) =>
                isAgentEnabled(flagStore, peer.id, id),
              ),
              reachable: statuses[index],
            },
          ]
        : [],
    ),
  ];

  const agentLabel = (id: string): string => agents.find((a) => a.id === id)?.label ?? id;

  // The displayed target: the desktop's own picks, falling back to the
  // roster's first agent when nothing is picked (local creates only).
  const focusedKey = nodeKey(desktop.node ?? undefined);
  const entry = entries.find((e) => e.node === focusedKey);
  const nodeMissing = desktop.node !== null && entry === undefined;
  // A parked stored pick still displays (the summary reads what was picked);
  // only the roster fallback skips parked agents.
  const agentId =
    desktop.agent ??
    (focusedKey === LOCAL_NODE_ID
      ? (agents.map((a) => a.id).find((id) => isAgentEnabled(flagStore, LOCAL_NODE_ID, id)) ?? null)
      : null);
  // Disabled peers drop out of `entries`, so a desktop aimed at one reads
  // as missing; a probed-and-failed peer is the other unreachable case.
  const unreachable = nodeMissing || entry?.reachable === false;
  const nodeLabel = entry?.label ?? nodeName(desktop.node ?? undefined);
  // Nothing reachable at all (local parked or down, every peer failed) —
  // the summary reports that instead of a stale node name.
  const noNodes = nodesConnected === "disconnected";
  // Probes in flight — the desktop summary isn't known yet, so the line
  // skeletons instead of flashing a default node·agent·model string.
  const checking = nodesConnected === "checking";

  // The dot tracks the desktop node's reachability — the local node's
  // own selfStatus (unknown until the first probe settles) or the peer's
  // probe result; a missing/disabled pick reads as offline.
  const dotState: "online" | "offline" | "checking" =
    noNodes || unreachable
      ? "offline"
      : focusedKey === LOCAL_NODE_ID
        ? selfStatus === "unknown"
          ? "checking"
          : "online"
        : entry === undefined || entry.reachable === undefined
          ? "checking"
          : "online";
  const dotLabel = noNodes
    ? "No nodes connected"
    : dotState === "online"
      ? `${nodeLabel} online`
      : dotState === "offline"
        ? `${nodeLabel} unreachable`
        : `Checking ${nodeLabel}`;

  // Summary segments — an unset pick dims to the default that will apply:
  // agent → the node's own pick (resolvable only as the local roster's
  // first), model → the agent's configured pref, then "agent default".
  const agentText =
    desktop.agent !== null
      ? agentLabel(desktop.agent)
      : agentId !== null
        ? agentLabel(agentId)
        : "node default";
  const agentPref = agentId === null ? undefined : modelPrefs[agentId];
  const configuredModel =
    agentPref !== undefined && agentPref.model.trim() !== "" ? agentPref.model.trim() : undefined;
  const modelText = desktop.model ?? configuredModel ?? "agent default";

  // The Agent submenu lists the focused node's roster; a stored pick that
  // fell out of it still shows so its check mark stays visible.
  const rosterAgents = entry?.agents ?? [];
  const agentItems =
    desktop.agent !== null && !rosterAgents.includes(desktop.agent)
      ? [desktop.agent, ...rosterAgents]
      : rosterAgents;

  // The Model submenu lists the agent's known models — the configured
  // pref plus its comma-separated fallbacks (Settings → Models), minus the
  // parked ones; a pick outside that list still shows so its check mark
  // stays visible (a stored-but-parked pick included — it just doesn't
  // apply at spawn).
  const knownModels = [
    ...new Set(
      [agentPref?.model, ...(agentPref?.fallbacks.split(",") ?? [])]
        .map((model) => model?.trim())
        .filter(
          (model): model is string =>
            model !== undefined &&
            model !== "" &&
            isModelEnabled(flagStore, focusedKey, agentId ?? "", model),
        ),
    ),
  ];
  // Parking the `auto` entry hides the "Agent default" affordance.
  const autoEnabled = isModelEnabled(flagStore, focusedKey, agentId ?? "", null);
  const modelItems =
    desktop.model !== null && !knownModels.includes(desktop.model)
      ? [desktop.model, ...knownModels]
      : knownModels;

  // The Directory submenu offers the desktop node's recent dirs — session
  // cwds are newest-first, so the first-seen order IS the recency order.
  const recentDirs = [
    ...new Set(
      sessions.filter((session) => nodeKey(session.node) === focusedKey).map((s) => s.cwd),
    ),
  ].slice(0, 6);
  const dirItems =
    desktop.cwd !== null && !recentDirs.includes(desktop.cwd)
      ? [desktop.cwd, ...recentDirs]
      : recentDirs;

  const pick = (node: string, agent: string | null): void => {
    const sameNode = nodeKey(desktop.node ?? undefined) === node;
    setFocus({
      node: node === LOCAL_NODE_ID ? null : node,
      agent,
      // Scoped picks don't port: a node change drops the dir too, an
      // agent change drops a model picked for the previous agent.
      model: sameNode && desktop.agent === agent ? desktop.model : null,
      cwd: sameNode ? desktop.cwd : null,
    });
  };
  const desktopSet =
    desktop.node !== null ||
    desktop.agent !== null ||
    desktop.model !== null ||
    desktop.cwd !== null;

  const initial = client?.label.trim().charAt(0).toUpperCase();

  return (
    <div className="border-t border-border p-2 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              className="h-auto w-full justify-start gap-2.5 px-2 py-1.5"
              aria-label="Client menu"
            />
          }
        >
          {client === null ? (
            <Skeleton className="size-8 shrink-0 rounded-full" />
          ) : (
            <span className="relative shrink-0">
              <span className="flex size-8 items-center justify-center rounded-full bg-secondary text-xs font-semibold text-secondary-foreground">
                {initial !== undefined && initial !== "" ? (
                  initial
                ) : (
                  <HugeiconsIcon icon={ComputerIcon} strokeWidth={2} />
                )}
              </span>
              <span
                role="status"
                aria-label={dotLabel}
                title={dotLabel}
                className={`absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-sidebar ${
                  dotState === "offline"
                    ? "animate-pulse bg-destructive"
                    : dotState === "checking"
                      ? "bg-muted-foreground/50"
                      : "bg-emerald-500"
                }`}
              />
            </span>
          )}
          <span className="min-w-0 flex-1 text-left">
            {client === null ? (
              <span className="block space-y-1.5">
                <Skeleton className="h-3.5 w-2/3" />
                <Skeleton className="h-3 w-1/3" />
              </span>
            ) : (
              <>
                <span className="block truncate text-sm font-medium">{client.label}</span>
                {checking ? (
                  <Skeleton className="mt-1 h-3 w-2/3" />
                ) : (
                  <span
                    className={`block truncate text-xs ${
                      unreachable ? "text-destructive/80" : "text-muted-foreground"
                    }`}
                  >
                    {noNodes ? (
                      "No nodes connected"
                    ) : unreachable ? (
                      `${nodeLabel} · unreachable`
                    ) : (
                      <>
                        {nodeLabel}
                        {" · "}
                        <span className={desktop.agent === null ? "opacity-60" : undefined}>
                          {agentText}
                        </span>
                        {" · "}
                        <span className={desktop.model === null ? "opacity-60" : undefined}>
                          {modelText}
                        </span>
                      </>
                    )}
                  </span>
                )}
              </>
            )}
          </span>
          <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          <DropdownMenuGroup>
            <DropdownMenuLabel>Desktop</DropdownMenuLabel>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger title="Node">
                <HugeiconsIcon
                  icon={ServerIcon}
                  strokeWidth={2}
                  className="shrink-0 text-muted-foreground"
                />
                <span className="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                  <span>Node</span>
                  <SubValue>{noNodes ? "No nodes connected" : nodeLabel}</SubValue>
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-44">
                {entries.map((target) => (
                  <DropdownMenuSub key={target.node}>
                    <DropdownMenuSubTrigger
                      className={target.reachable === false ? "text-muted-foreground" : undefined}
                    >
                      {target.label}
                      {focusedKey === target.node && desktop.agent === null && <Check />}
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="w-44">
                      <DropdownMenuItem onClick={() => pick(target.node, null)}>
                        Node default
                        {focusedKey === target.node && desktop.agent === null && <Check />}
                      </DropdownMenuItem>
                      {target.agents.length > 0 && <DropdownMenuSeparator />}
                      {target.agents.length === 0 && (
                        <DropdownMenuItem disabled>
                          <span className="text-muted-foreground">No agents</span>
                        </DropdownMenuItem>
                      )}
                      {target.agents.map((id) => (
                        <DropdownMenuItem key={id} onClick={() => pick(target.node, id)}>
                          {agentLabel(id)}
                          {focusedKey === target.node && desktop.agent === id && <Check />}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                ))}
                {nodeMissing && desktop.node !== null && (
                  <DropdownMenuItem disabled>
                    {nodeName(desktop.node)} — unavailable
                  </DropdownMenuItem>
                )}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger title="Agent">
                <HugeiconsIcon
                  icon={BotIcon}
                  strokeWidth={2}
                  className="shrink-0 text-muted-foreground"
                />
                <span className="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                  <span>Agent</span>
                  <SubValue>
                    {desktop.agent === null ? "Node default" : agentLabel(desktop.agent)}
                  </SubValue>
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-44">
                <DropdownMenuItem onClick={() => pick(focusedKey, null)}>
                  Node default
                  {desktop.agent === null && <Check />}
                </DropdownMenuItem>
                {agentItems.length > 0 && <DropdownMenuSeparator />}
                {agentItems.map((id) => (
                  <DropdownMenuItem key={id} onClick={() => pick(focusedKey, id)}>
                    {agentLabel(id)}
                    {desktop.agent === id && <Check />}
                  </DropdownMenuItem>
                ))}
                {agentItems.length === 0 && (
                  <DropdownMenuItem disabled>
                    <span className="text-muted-foreground">No agents</span>
                  </DropdownMenuItem>
                )}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger title="Model">
                <HugeiconsIcon
                  icon={BrainIcon}
                  strokeWidth={2}
                  className="shrink-0 text-muted-foreground"
                />
                <span className="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                  <span>Model</span>
                  <SubValue>{modelText}</SubValue>
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-52">
                {autoEnabled && (
                  <DropdownMenuItem onClick={() => setFocus({ model: null })}>
                    Agent default
                    {desktop.model === null && <Check />}
                  </DropdownMenuItem>
                )}
                {modelItems.length > 0 && <DropdownMenuSeparator />}
                {modelItems.map((model) => (
                  <DropdownMenuItem key={model} onClick={() => setFocus({ model })}>
                    <span className="min-w-0 flex-1 truncate">{model}</span>
                    {desktop.model === model && <Check />}
                  </DropdownMenuItem>
                ))}
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => setSettingsOpen(true, "desktop")}>
                  Custom…
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            <DropdownMenuSub>
              <DropdownMenuSubTrigger title="Working directory">
                <HugeiconsIcon
                  icon={FolderIcon}
                  strokeWidth={2}
                  className="shrink-0 text-muted-foreground"
                />
                <span className="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                  <span>Directory</span>
                  <SubValue>
                    {desktop.cwd === null ? "Most recent" : dirLabel(desktop.cwd)}
                  </SubValue>
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-56">
                <DropdownMenuItem onClick={() => setFocus({ cwd: null })}>
                  Most recent
                  {desktop.cwd === null && <Check />}
                </DropdownMenuItem>
                {dirItems.length > 0 && <DropdownMenuSeparator />}
                {dirItems.map((dir) => (
                  <DropdownMenuItem key={dir} title={dir} onClick={() => setFocus({ cwd: dir })}>
                    <span className="min-w-0 flex-1 truncate">{dirLabel(dir)}</span>
                    {desktop.cwd === dir && <Check />}
                  </DropdownMenuItem>
                ))}
                {dirItems.length === 0 && (
                  <DropdownMenuItem disabled>
                    <span className="text-muted-foreground">No known directories</span>
                  </DropdownMenuItem>
                )}
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => setSettingsOpen(true, "desktop")}>
                  Custom…
                </DropdownMenuItem>
              </DropdownMenuSubContent>
            </DropdownMenuSub>
            {desktopSet && <DropdownMenuItem onClick={clearFocus}>Reset desktop</DropdownMenuItem>}
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setSettingsOpen(true)}>
            <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} />
            Settings
          </DropdownMenuItem>
          {hasKeyboard && (
            <DropdownMenuItem onClick={() => setSettingsOpen(true, "keyboard")}>
              <HugeiconsIcon icon={KeyboardIcon} strokeWidth={2} />
              Keyboard shortcuts
              <DropdownMenuShortcut>?</DropdownMenuShortcut>
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <SettingsDialog open={settingsOpen} onOpenChange={openSettings} />
    </div>
  );
}
