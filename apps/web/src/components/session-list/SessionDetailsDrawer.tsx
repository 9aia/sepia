import { useEffect, useRef, useState } from "react";
import {
  ArrowReloadHorizontalIcon,
  Cancel01Icon,
  Copy01Icon,
  Delete02Icon,
  Edit02Icon,
  FolderLibraryIcon,
  FolderOpenIcon,
  Globe02Icon,
  PinIcon,
  Tick02Icon,
} from "@hugeicons/core-free-icons";
import { ChevronDownIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../../lib/types";
import { nodeKey, projectKey, resolveSession, sessionKey } from "../../lib/format";
import { nodeName, spanNodeLabel } from "../../lib/nodes";
import { useNodes } from "../../hooks/query/useNodes";
import { useSessions } from "../../hooks/query/useSessions";
import { useRenameSession } from "../../hooks/query/useRenameSession";
import { useAgents } from "../../hooks/query/useAgents";
import { usePatchSessionMeta } from "../../hooks/query/useSessionMeta";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { useConvertSession, useProjects } from "../../hooks/query/useProjects";
import { useResumeSession, useResumeTargets } from "../../hooks/query/useResumeSession";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../ui/alert-dialog";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerTitle,
} from "../ui/drawer";
import { Input } from "../ui/input";

function Detail({
  label,
  copyValue,
  children,
}: {
  readonly label: string;
  readonly copyValue?: string;
  readonly children: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  const copy = (): void => {
    if (copyValue === undefined) return;
    void navigator.clipboard.writeText(copyValue).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      },
      () => undefined,
    );
  };
  return (
    <div className="flex flex-col gap-0.5 py-2 text-sm">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="flex min-w-0 items-center gap-1">
        <span className="min-w-0 flex-1 truncate" title={copyValue}>
          {children}
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

function RenameDialog({
  session,
  open,
  onOpenChange,
}: {
  readonly session: SessionSummary;
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const renameMutation = useRenameSession();
  const [title, setTitle] = useState(session.title);
  const titleRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setTitle(session.title);
    requestAnimationFrame(() => {
      titleRef.current?.focus();
      titleRef.current?.select();
    });
  }, [open, session.title]);

  const dirty = title.trim() !== "" && title.trim() !== session.title;
  const rename = (): void => {
    if (!dirty) return;
    renameMutation.mutate(
      { id: session.id, title: title.trim(), agent: session.agent, node: session.node },
      { onSuccess: () => onOpenChange(false) },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Rename session</DialogTitle>
          <DialogDescription>
            Saved as a Sepia overlay — the agent&apos;s own store is read-only.
          </DialogDescription>
        </DialogHeader>
        <Input
          ref={titleRef}
          value={title}
          aria-label="Session title"
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") rename();
          }}
        />
        {renameMutation.isError && (
          <p className="text-xs text-destructive">
            {renameMutation.error instanceof Error
              ? renameMutation.error.message
              : "Failed to rename session"}
          </p>
        )}
        <DialogFooter>
          <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
          <Button disabled={!dirty || renameMutation.isPending} onClick={rename}>
            Rename
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface SessionDetailsDrawerProps {
  readonly session: SessionSummary | undefined;
  /** Focus the title field on open (context-menu "Rename…"). */
  readonly focusRename: boolean;
  onClose: () => void;
  onOpen: (id: string) => void;
  onDelete: (session: SessionSummary) => void;
}

export function SessionDetailsDrawer({
  session,
  focusRename,
  onClose,
  onOpen,
  onDelete,
}: SessionDetailsDrawerProps) {
  const patch = usePatchSessionMeta();
  const convert = useConvertSession();
  const resume = useResumeSession();
  const resumeNodes = useResumeTargets();
  const { data: projects } = useProjects();
  const { data: agents = [] } = useAgents();
  const { data: sessions = [] } = useSessions();
  // Subscribing here also refreshes node labels when the peer registry lands.
  const { peers } = useNodes();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);
  const peer = session === undefined ? undefined : peers.find((p) => p.id === session.node);
  // The sub-agent's parent — clickable when the row is in the merged list.
  const parent =
    session === undefined ? undefined : resolveSession(sessions, session.parentSessionId);

  useEffect(() => {
    if (session !== undefined && focusRename) setRenameOpen(true);
  }, [session, focusRename]);

  return (
    <Drawer
      swipeDirection="right"
      open={session !== undefined}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DrawerContent className="[--drawer-content-height:calc(100dvh-var(--drawer-inset,0px)*2)] [--drawer-content-width:22rem]">
        {session !== undefined && (
          <>
            <DrawerHeader className="flex-row items-start justify-between">
              <div>
                <DrawerTitle>Session details</DrawerTitle>
                <DrawerDescription>The underlying agent store is read-only.</DrawerDescription>
              </div>
              <DrawerClose
                render={<Button variant="ghost" size="icon-xs" aria-label="Close details" />}
              >
                <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
              </DrawerClose>
            </DrawerHeader>

            <div className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto px-4 py-3">
              <div className="mt-2 divide-y divide-border/50">
                <Detail label="Last active">{new Date(session.updatedAt).toLocaleString()}</Detail>
                <Detail label="Status">
                  {session.locked ? (
                    <Badge
                      variant="destructive"
                      title={`Locked by pid ${session.lockHolderPid ?? "unknown"}`}
                    >
                      locked
                    </Badge>
                  ) : session.busy ? (
                    <Badge variant="destructive">busy</Badge>
                  ) : (
                    <Badge variant="secondary">free</Badge>
                  )}
                </Detail>
                <Detail label="Server">
                  {nodeName(session.node)}
                  {peer !== undefined && (
                    <span className="text-muted-foreground">{` · ${new URL(peer.url).host}`}</span>
                  )}
                </Detail>
              </div>
              <Collapsible className="mt-2">
                <CollapsibleTrigger className="flex w-full items-center justify-between rounded-md px-1 py-2 text-xs font-medium text-muted-foreground hover:text-foreground">
                  Advanced
                  <HugeiconsIcon
                    icon={ChevronDownIcon}
                    strokeWidth={2}
                    className="size-3.5 transition-transform group-data-open/collapsible:rotate-180"
                  />
                </CollapsibleTrigger>
                <CollapsibleContent className="divide-y divide-border/50">
                  <Detail label="ID" copyValue={session.id}>
                    <span className="font-mono text-xs">{session.id}</span>
                  </Detail>
                  <Detail label="Working directory" copyValue={session.cwd}>
                    <span className="font-mono text-xs" title={session.cwd}>
                      {session.cwd}
                    </span>
                  </Detail>
                  <Detail label="Agent">
                    <Badge variant="secondary">{session.agent}</Badge>
                    {session.agentId !== undefined && session.agentId !== "" && (
                      <span
                        className="text-muted-foreground"
                        title="Sub-agent identity within the parent's team"
                      >
                        · {session.agentId}
                      </span>
                    )}
                  </Detail>
                  {session.parentSessionId !== undefined && session.parentSessionId !== "" && (
                    <Detail label="Parent" copyValue={session.parentSessionId}>
                      {parent !== undefined ? (
                        <button
                          type="button"
                          className="font-mono text-xs text-primary underline-offset-4 hover:underline"
                          title={`Open ${parent.title}`}
                          onClick={() => {
                            onOpen(sessionKey(parent));
                            onClose();
                          }}
                        >
                          {session.parentSessionId}
                        </button>
                      ) : (
                        <span className="font-mono text-xs">{session.parentSessionId}</span>
                      )}
                    </Detail>
                  )}
                  <Detail label="Source">{session.source}</Detail>
                  {session.spans.length > 0 && (
                    <div className="flex flex-col gap-0.5 py-2 text-sm">
                      <span className="text-xs text-muted-foreground">Runs</span>
                      <div className="flex flex-col gap-1.5">
                        {session.spans.map((span, index) => (
                          <span
                            key={`${span.at}-${index}`}
                            className="flex items-center gap-1.5 text-xs"
                          >
                            <Badge variant="secondary" className="font-normal">
                              {span.agent} @ {spanNodeLabel(span.node)}
                            </Badge>
                            <span className="text-muted-foreground">
                              {new Date(span.at).toLocaleString()}
                            </span>
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                </CollapsibleContent>
              </Collapsible>
            </div>

            <DrawerFooter className="flex-col gap-2">
              <Button
                className="w-full justify-start"
                onClick={() => {
                  onOpen(sessionKey(session));
                  onClose();
                }}
              >
                <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
                Open session
              </Button>
              <Button
                variant="outline"
                className="w-full justify-start"
                onClick={() =>
                  patch.mutate({
                    id: session.id,
                    agent: session.agent,
                    node: session.node,
                    patch: { pinned: !session.pinned },
                  })
                }
              >
                <HugeiconsIcon icon={PinIcon} strokeWidth={2} />
                {session.pinned === true ? "Unpin" : "Pin"}
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button variant="outline" className="w-full justify-start">
                      <HugeiconsIcon icon={FolderLibraryIcon} strokeWidth={2} />
                      Projects…
                    </Button>
                  }
                />
                <DropdownMenuContent align="start" className="w-(--anchor-width)">
                  {projects.filter((p) => nodeKey(p.node) === nodeKey(session.node)).length ===
                    0 && (
                    <DropdownMenuItem disabled>
                      <span className="text-muted-foreground">No projects yet</span>
                    </DropdownMenuItem>
                  )}
                  {projects
                    .filter((p) => nodeKey(p.node) === nodeKey(session.node))
                    .map((project) => (
                      <DropdownMenuItem
                        key={projectKey(project)}
                        closeOnClick={false}
                        onClick={() => {
                          const key = projectKey(project);
                          const ids = session.projectIds.includes(key)
                            ? session.projectIds.filter((p) => p !== key)
                            : [...session.projectIds, key];
                          patch.mutate({
                            id: session.id,
                            agent: session.agent,
                            node: session.node,
                            patch: { projectIds: ids },
                          });
                        }}
                      >
                        {project.name}
                        {session.projectIds.includes(projectKey(project)) && (
                          <span className="ml-auto text-xs text-primary">✓</span>
                        )}
                      </DropdownMenuItem>
                    ))}
                </DropdownMenuContent>
              </DropdownMenu>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button variant="outline" className="w-full justify-start">
                      <HugeiconsIcon icon={ArrowReloadHorizontalIcon} strokeWidth={2} />
                      Convert to…
                    </Button>
                  }
                />
                <DropdownMenuContent align="start" className="w-(--anchor-width)">
                  {agents
                    .filter((a) => a.id !== session.agent)
                    .map((agent) => (
                      <DropdownMenuItem
                        key={agent.id}
                        onClick={() =>
                          convert.mutate({
                            id: session.id,
                            agent: agent.id,
                            fromAgent: session.agent,
                            node: session.node,
                          })
                        }
                      >
                        {agent.label}
                      </DropdownMenuItem>
                    ))}
                </DropdownMenuContent>
              </DropdownMenu>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      variant="outline"
                      className="w-full justify-start"
                      disabled={resume.isPending}
                    >
                      <HugeiconsIcon icon={Globe02Icon} strokeWidth={2} />
                      Resume on…
                    </Button>
                  }
                />
                <DropdownMenuContent align="start" className="w-(--anchor-width)">
                  {resumeNodes.map((entry) => {
                    const targets = entry.agents.filter(
                      (id) =>
                        !(nodeKey(entry.node) === nodeKey(session.node) && id === session.agent),
                    );
                    return (
                      <DropdownMenuSub key={entry.node ?? "local"}>
                        <DropdownMenuSubTrigger>{entry.label}</DropdownMenuSubTrigger>
                        <DropdownMenuSubContent className="w-40">
                          {targets.length === 0 && (
                            <DropdownMenuItem disabled>
                              <span className="text-muted-foreground">No other agents</span>
                            </DropdownMenuItem>
                          )}
                          {targets.map((agentId) => (
                            <DropdownMenuItem
                              key={agentId}
                              disabled={resume.isPending}
                              onClick={() =>
                                resume.mutate({
                                  session,
                                  agent: agentId,
                                  node: entry.node,
                                })
                              }
                            >
                              {agents.find((a) => a.id === agentId)?.label ?? agentId}
                            </DropdownMenuItem>
                          ))}
                        </DropdownMenuSubContent>
                      </DropdownMenuSub>
                    );
                  })}
                </DropdownMenuContent>
              </DropdownMenu>
              <Button
                variant="outline"
                className="w-full justify-start"
                onClick={() => setRenameOpen(true)}
              >
                <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
                Rename…
              </Button>
              <Button
                variant="destructive"
                className="w-full justify-start"
                onClick={() => setConfirmOpen(true)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                Delete
              </Button>
            </DrawerFooter>
            <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Delete session?</AlertDialogTitle>
                  <AlertDialogDescription>
                    {`"${session.title}" will be permanently removed from the agent's session store. This can't be undone.`}
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Cancel</AlertDialogCancel>
                  <AlertDialogAction
                    variant="destructive"
                    onClick={() => {
                      onDelete(session);
                      setConfirmOpen(false);
                      onClose();
                    }}
                  >
                    Delete
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
            <RenameDialog session={session} open={renameOpen} onOpenChange={setRenameOpen} />
          </>
        )}
      </DrawerContent>
    </Drawer>
  );
}
