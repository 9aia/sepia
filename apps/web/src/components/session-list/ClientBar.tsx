import { useStore } from "@tanstack/react-store";
import { useHasKeyboard } from "../../lib/keyboard";
import {
  ArrowUp01Icon,
  ComputerIcon,
  KeyboardIcon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useHealth } from "../../hooks/query/useHealth";
import { useAppHotkey } from "../../lib/keybinds";
import { sepiaStore, setFocus, setSettingsOpen, type FocusTarget } from "../../lib/store";
import { useClient } from "../../lib/client";
import { useFocus } from "../../lib/focus";
import { isPeerEnabled, nodeName } from "../../lib/nodes";
import { LOCAL_NODE_ID, nodeKey } from "../../lib/format";
import { defaultAgentFor, settingsStore } from "../../lib/settings";
import { useAgents } from "../../hooks/query/useAgents";
import { useNodes, useNodeStatuses, usePeerDescriptors } from "../../hooks/query/useNodes";
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

/** One focusable machine — this node first, then every enabled peer. */
interface FocusEntry {
  /** nodeKey — "local" or the peer's registered id. */
  readonly node: string;
  readonly label: string;
  readonly agents: ReadonlyArray<string>;
  /** false = probed and failed; undefined = not probed yet. */
  readonly reachable: boolean | undefined;
}

/** The check mark shared by the focus menu's current picks. */
const FocusCheck = () => <span className="ml-auto text-xs text-primary">✓</span>;

/**
 * The sidebar footer: this client (browser/device identity — label +
 * keypair, managed in Settings → Client) plus the *focus* line naming the
 * machine+agent "New session" targets. The menu's Focus section picks a
 * node submenu → agent; a focused node that goes offline or gets disabled
 * dims and reads "unreachable" until the user refocuses.
 */
export function ClientBar() {
  const hasKeyboard = useHasKeyboard();
  const client = useClient();
  const focus = useFocus();
  const health = useHealth();
  const settings = useStore(settingsStore);
  const { self, selfStatus, peers } = useNodes();
  const statuses = useNodeStatuses(peers);
  const descriptors = usePeerDescriptors(peers);
  const { data: agents = [] } = useAgents();
  const settingsOpen = useStore(sepiaStore, (state) => state.settingsOpen);
  useAppHotkey("app.settings", () => setSettingsOpen(!settingsOpen));
  const openSettings = (open: boolean): void => setSettingsOpen(open);

  // The merged roster stands in for a node whose descriptor hasn't landed.
  const fallbackIds = agents.map((agent) => agent.id);
  const entries: FocusEntry[] = [
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

  // The displayed target: the explicit focus, else the default chain —
  // this machine plus its configured default (or the roster's first agent).
  const focusedKey = nodeKey(focus?.node);
  const entry = entries.find((e) => e.node === focusedKey);
  const focusMissing = focus !== null && entry === undefined;
  const agentId =
    focus === null
      ? (defaultAgentFor(settings, undefined) ?? agents[0]?.id ?? null)
      : (focus.agent ??
        defaultAgentFor(settings, focus.node) ??
        (focusedKey === LOCAL_NODE_ID ? (agents[0]?.id ?? null) : null));
  // Disabled peers drop out of `entries`, so a focus on one reads as
  // missing; a probed-and-failed peer is the other unreachable case.
  const unreachable = focusMissing || entry?.reachable === false;
  const focusLine = unreachable
    ? `${entry?.label ?? nodeName(focus?.node)} · unreachable`
    : `${entry?.label ?? "local"} · ${agentId === null ? "auto" : agentLabel(agentId)}`;

  const pick = (node: string, agent: string | null): void => {
    const target: FocusTarget = { node, agent };
    setFocus(target);
  };

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
                aria-label={health.isError ? "Server unreachable" : "Server online"}
                title={health.isError ? "Server unreachable" : "Server online"}
                className={`absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-sidebar ${
                  health.isError ? "animate-pulse bg-destructive" : "bg-emerald-500"
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
                  {focusLine}
                </span>
              </>
            )}
          </span>
          <HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-60">
          <DropdownMenuGroup>
            <DropdownMenuLabel>New sessions target</DropdownMenuLabel>
            {entries.map((target) => (
              <DropdownMenuSub key={target.node}>
                <DropdownMenuSubTrigger
                  className={target.reachable === false ? "text-muted-foreground" : undefined}
                >
                  {target.label}
                  {focus?.node === target.node && focus.agent === null && <FocusCheck />}
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="w-44">
                  <DropdownMenuItem onClick={() => pick(target.node, null)}>
                    Node default
                    {focus?.node === target.node && focus.agent === null && <FocusCheck />}
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
                      {focus?.node === target.node && focus.agent === id && <FocusCheck />}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            ))}
            {focusMissing && (
              <DropdownMenuItem disabled>{nodeName(focus.node)} — unavailable</DropdownMenuItem>
            )}
            {focus !== null && (
              <DropdownMenuItem onClick={() => setFocus(null)}>Clear focus</DropdownMenuItem>
            )}
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
