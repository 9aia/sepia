import type { ReactNode } from "react";
import { useStore } from "@tanstack/react-store";
import { useHasKeyboard } from "../../lib/keyboard";
import {
  ArrowUp01Icon,
  ComputerIcon,
  KeyboardIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useAppHotkey } from "../../lib/keybinds";
import { sepiaStore, setSettingsOpen } from "../../lib/store";
import { settingsStore } from "../../lib/settings";
import { useClient } from "../../lib/client";
import { clearFocus, setFocus, useFocus } from "../../lib/focus";
import { isPeerEnabled, nodeName } from "../../lib/nodes";
import { LOCAL_NODE_ID, nodeKey } from "../../lib/format";
import { useAgents } from "../../hooks/query/useAgents";
import { useNodes, useNodeStatuses, usePeerDescriptors } from "../../hooks/query/useNodes";
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

/** One targetable machine — this node first, then every enabled peer. */
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
 * the machine+agent+model "New session" aims at, the same
 * `settings.desktop` Settings → Desktop edits. The dot tracks the
 * desktop machine's reachability (selfStatus for this node, the peer
 * probe otherwise). The menu edits the desktop: a node submenu per
 * enabled machine picks node+agent, then Model and Directory submenus
 * cover the remaining fields. A desktop node that goes offline or gets
 * disabled dims and reads "unreachable" until the user repicks.
 */
export function ClientBar() {
  const hasKeyboard = useHasKeyboard();
  const client = useClient();
  const desktop = useFocus();
  const modelPrefs = useStore(settingsStore, (state) => state.models);
  const { self, selfStatus, peers } = useNodes();
  const statuses = useNodeStatuses(peers);
  const descriptors = usePeerDescriptors(peers);
  const { data: agents = [] } = useAgents();
  const { data: sessions = [] } = useSessions();
  const settingsOpen = useStore(sepiaStore, (state) => state.settingsOpen);
  useAppHotkey("app.settings", () => setSettingsOpen(!settingsOpen));
  const openSettings = (open: boolean): void => setSettingsOpen(open);

  // The merged roster stands in for a node whose descriptor hasn't landed.
  const fallbackIds = agents.map((agent) => agent.id);
  const entries: DesktopEntry[] = [
    {
      node: LOCAL_NODE_ID,
      label: nodeName(undefined),
      agents: self?.agents ?? fallbackIds,
      reachable: selfStatus !== "offline",
    },
    ...peers.flatMap((peer, index) =>
      isPeerEnabled(peer)
        ? [
            {
              node: peer.id,
              label: peer.alias ?? peer.name,
              agents: descriptors[index]?.agents ?? fallbackIds,
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
  const agentId = desktop.agent ?? (focusedKey === LOCAL_NODE_ID ? (agents[0]?.id ?? null) : null);
  // Disabled peers drop out of `entries`, so a desktop aimed at one reads
  // as missing; a probed-and-failed peer is the other unreachable case.
  const unreachable = nodeMissing || entry?.reachable === false;
  const machineLabel = entry?.label ?? nodeName(desktop.node ?? undefined);

  // The dot tracks the desktop machine's reachability — the local node's
  // own selfStatus (unknown until the first probe settles) or the peer's
  // probe result; a missing/disabled pick reads as offline.
  const dotState: "online" | "offline" | "checking" = unreachable
    ? "offline"
    : focusedKey === LOCAL_NODE_ID
      ? selfStatus === "unknown"
        ? "checking"
        : "online"
      : entry === undefined || entry.reachable === undefined
        ? "checking"
        : "online";
  const dotLabel =
    dotState === "online"
      ? `${machineLabel} online`
      : dotState === "offline"
        ? `${machineLabel} unreachable`
        : `Checking ${machineLabel}`;

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

  // The Model submenu lists the agent's known models — the configured
  // pref plus its comma-separated fallbacks (Settings → Models); a pick
  // outside that list still shows so its check mark stays visible.
  const knownModels = [
    ...new Set(
      [agentPref?.model, ...(agentPref?.fallbacks.split(",") ?? [])]
        .map((model) => model?.trim())
        .filter((model): model is string => model !== undefined && model !== ""),
    ),
  ];
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
      // Scoped picks don't port: a machine change drops the dir too, an
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
                <span
                  className={`block truncate text-xs ${
                    unreachable ? "text-destructive/80" : "text-muted-foreground"
                  }`}
                >
                  {unreachable ? (
                    `${machineLabel} · unreachable`
                  ) : (
                    <>
                      {machineLabel}
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
              </>
            )}
          </span>
          <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          <DropdownMenuGroup>
            <DropdownMenuLabel>Desktop</DropdownMenuLabel>
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
              <DropdownMenuItem disabled>{nodeName(desktop.node)} — unavailable</DropdownMenuItem>
            )}
            <DropdownMenuSeparator />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <span className="flex min-w-0 flex-1 items-baseline justify-between gap-2">
                  <span>Model</span>
                  <SubValue>{modelText}</SubValue>
                </span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-52">
                <DropdownMenuItem onClick={() => setFocus({ model: null })}>
                  Agent default
                  {desktop.model === null && <Check />}
                </DropdownMenuItem>
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
              <DropdownMenuSubTrigger>
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
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => setSettingsOpen(true, "keyboard")}>
                <HugeiconsIcon icon={KeyboardIcon} strokeWidth={2} />
                Keyboard shortcuts
                <DropdownMenuShortcut>?</DropdownMenuShortcut>
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <SettingsDialog open={settingsOpen} onOpenChange={openSettings} />
    </div>
  );
}
