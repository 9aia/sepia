import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAppHotkey } from "../../lib/keybinds";
import { useTree } from "@headless-tree/react";
import { syncDataLoaderFeature } from "@headless-tree/core";
import type { ItemInstance } from "@headless-tree/core";
import type { SessionSummary } from "../../lib/types";
import {
  formatUpdated,
  nodeKey,
  projectKey,
  projectName,
  resolveSession,
  sessionKey,
} from "../../lib/format";
import {
  SESSION_TREE_ROOT,
  sessionDirId,
  treeFromSessions,
  type SessionTreeData,
} from "../../lib/sessionTree";
import { nodeName } from "../../lib/nodes";
import { usePatchSessionMeta } from "../../hooks/query/useSessionMeta";
import { useCreateProject } from "../../hooks/query/useProjects";
import { useMultiNode, useNodeLabel } from "../../hooks/query/useNodes";
import { useUiState } from "../../hooks/query/useConfig";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  Copy01Icon,
  FolderDetailsIcon,
  FolderLibraryIcon,
  MoreVerticalIcon,
} from "@hugeicons/core-free-icons";
import { SessionActions } from "./SessionActions";
import { NodeBadge } from "./NodeBadge";
import { Tree, TreeItem, TreeItemLabel } from "../reui/tree";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../ui/dialog";
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

type TreeData = SessionTreeData;

const ROOT_ID = SESSION_TREE_ROOT;

/** Shared action items for a directory row (context menu + ⋯ dropdown). */
function DirActions({
  Item,
  data,
  sessions,
  onShowDetails,
  onNewSession,
}: {
  readonly Item: typeof ContextMenuItem;
  readonly data: TreeData & { kind: "dir" };
  readonly sessions: ReadonlyArray<SessionSummary>;
  onShowDetails: () => void;
  onNewSession: (cwd: string, node?: string) => void;
}) {
  const createProject = useCreateProject();
  const patch = usePatchSessionMeta();
  // In grouped mode dirs carry their node — same-path folders on different
  // machines are separate rows, so membership must match node too.
  const members = sessions.filter(
    (session) =>
      nodeKey(session.node) === nodeKey(data.node) &&
      (session.cwd === data.cwd || session.cwd.startsWith(`${data.cwd}/`)),
  );
  const createFromFolder = (): void => {
    // Members are already same-node (grouped dirs filter by it) — create the
    // project on that node and only enroll same-node rows (projects are
    // node-local).
    const node = members[0]?.node;
    createProject.mutate(
      { name: projectName(data.cwd), node },
      {
        onSuccess: ({ project }) => {
          const key = projectKey(project);
          for (const session of members) {
            if (nodeKey(session.node) !== nodeKey(project.node)) continue;
            if (session.projectIds.includes(key)) continue;
            patch.mutate({
              id: session.id,
              agent: session.agent,
              node: session.node,
              patch: { projectIds: [...session.projectIds, key] },
            });
          }
        },
      },
    );
  };
  const copy = (): void => {
    void navigator.clipboard.writeText(data.cwd).then(
      () => {},
      () => undefined,
    );
  };
  return (
    <>
      <Item onClick={() => onNewSession(data.cwd, data.node)}>
        <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
        New session here
      </Item>
      <Item onClick={createFromFolder} disabled={members.length === 0}>
        <HugeiconsIcon icon={FolderLibraryIcon} strokeWidth={2} />
        New project from folder
      </Item>
      <Item onClick={onShowDetails}>
        <HugeiconsIcon icon={FolderDetailsIcon} strokeWidth={2} />
        Folder details
      </Item>
      <Item onClick={copy}>
        <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
        Copy path
      </Item>
    </>
  );
}

function DirDetailsDialog({
  data,
  open,
  onOpenChange,
}: {
  readonly data: TreeData & { kind: "dir" };
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  // Grouped dirs carry their owning node — surface it so same-path folders
  // on two machines stay distinguishable here too.
  const nodeLabel = useNodeLabel(data.node);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Folder details</DialogTitle>
          <DialogDescription>Sessions are grouped under this directory.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-2 text-sm">
          <div className="flex items-center justify-between gap-4">
            <span className="text-muted-foreground">Path</span>
            <span className="min-w-0 text-right font-mono text-xs break-all" title={data.cwd}>
              {data.cwd}
            </span>
          </div>
          {data.node !== undefined && (
            <div className="flex items-center justify-between gap-4">
              <span className="text-muted-foreground">Node</span>
              <span>{nodeLabel}</span>
            </div>
          )}
          <div className="flex items-center justify-between gap-4">
            <span className="text-muted-foreground">Sessions</span>
            <span>{data.count}</span>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function GroupRow({
  item,
  data,
  sessions,
  onNewSession,
}: {
  item: ItemInstance<TreeData>;
  data: TreeData & { kind: "dir" };
  sessions: ReadonlyArray<SessionSummary>;
  onNewSession: (cwd: string, node?: string) => void;
}) {
  const [detailsOpen, setDetailsOpen] = useState(false);
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger className="relative block">
          <TreeItem item={item} className="w-full">
            <TreeItemLabel className="rounded-md bg-transparent hover:bg-accent/60">
              <span className="flex-1 truncate font-semibold" title={data.cwd}>
                {data.label}
              </span>
            </TreeItemLabel>
          </TreeItem>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="absolute top-1/2 right-2 -translate-y-1/2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
                  aria-label={`Actions for folder ${data.label}`}
                  title="More actions"
                />
              }
              onClick={(event) => event.stopPropagation()}
            >
              <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <DirActions
                Item={DropdownMenuItem as unknown as typeof ContextMenuItem}
                data={data}
                sessions={sessions}
                onShowDetails={() => setDetailsOpen(true)}
                onNewSession={onNewSession}
              />
            </DropdownMenuContent>
          </DropdownMenu>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <DirActions
            Item={ContextMenuItem}
            data={data}
            sessions={sessions}
            onShowDetails={() => setDetailsOpen(true)}
            onNewSession={onNewSession}
          />
        </ContextMenuContent>
      </ContextMenu>
      <DirDetailsDialog data={data} open={detailsOpen} onOpenChange={setDetailsOpen} />
    </>
  );
}

/** Top-level machine group in multi-node mode — just a labeled folder row. */
function NodeRow({
  item,
  data,
}: {
  item: ItemInstance<TreeData>;
  data: TreeData & { kind: "node" };
}) {
  const label = useNodeLabel(data.node);
  return (
    <TreeItem item={item} className="w-full">
      <TreeItemLabel className="rounded-md bg-transparent hover:bg-accent/60">
        <span className="flex-1 truncate font-semibold" title={label}>
          {label}
        </span>
        <span className="text-xs text-muted-foreground">{data.count}</span>
      </TreeItemLabel>
    </TreeItem>
  );
}

function SessionItemRow({
  item,
  session,
  selected,
  onSelect,
  onDetails,
  onDelete,
}: {
  readonly item: ItemInstance<TreeData>;
  readonly session: SessionSummary;
  readonly selected: boolean;
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (session: SessionSummary) => void;
}) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger className="relative block">
          <TreeItem
            item={item}
            data-selected={selected || undefined}
            className="w-full cursor-pointer border-0 bg-transparent p-0 text-left font-[inherit] text-inherit"
          >
            <TreeItemLabel className="w-full items-start rounded-md bg-transparent hover:bg-accent/60 in-data-[selected=true]:ring-1 in-data-[selected=true]:ring-inset in-data-[selected=true]:ring-primary">
              <div className="min-w-0 flex-1">
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate font-semibold" title={session.title}>
                    {session.parentSessionId !== undefined && session.parentSessionId !== "" && (
                      <span className="font-normal text-muted-foreground" title="Sub-agent session">
                        ↳{" "}
                      </span>
                    )}
                    {session.title}
                  </span>
                </div>
                <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                  <span>{formatUpdated(session.updatedAt)}</span>
                  <NodeBadge node={session.node} />
                  {session.locked && (
                    <Badge
                      variant="destructive"
                      title={`Locked by pid ${session.lockHolderPid ?? "unknown"}`}
                    >
                      locked
                    </Badge>
                  )}
                </div>
              </div>
            </TreeItemLabel>
          </TreeItem>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="absolute top-2 right-2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
                  aria-label={`Actions for session ${session.title}`}
                  title="More actions"
                />
              }
              onClick={(event) => event.stopPropagation()}
            >
              <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <SessionActions
                Item={DropdownMenuItem as unknown as typeof ContextMenuItem}
                Separator={DropdownMenuSeparator}
                Sub={DropdownMenuSub as unknown as typeof ContextMenuSub}
                SubTrigger={DropdownMenuSubTrigger as unknown as typeof ContextMenuSubTrigger}
                SubContent={DropdownMenuSubContent as unknown as typeof ContextMenuSubContent}
                session={session}
                onSelect={onSelect}
                onDetails={onDetails}
                onRequestDelete={() => setConfirmOpen(true)}
              />
            </DropdownMenuContent>
          </DropdownMenu>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <SessionActions
            Item={ContextMenuItem}
            Separator={ContextMenuSeparator}
            Sub={ContextMenuSub}
            SubTrigger={ContextMenuSubTrigger}
            SubContent={ContextMenuSubContent}
            session={session}
            onSelect={onSelect}
            onDetails={onDetails}
            onRequestDelete={() => setConfirmOpen(true)}
          />
        </ContextMenuContent>
      </ContextMenu>
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
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

interface SessionTreeProps {
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  /** Shared sidebar scroller the tree virtualizes against. */
  readonly scrollRef: RefObject<HTMLDivElement | null>;
  readonly hotkeyTarget: RefObject<HTMLElement | null>;
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (session: SessionSummary) => void;
  onNewSession: (cwd: string, node?: string) => void;
}

export function SessionTree({
  sessions,
  selectedId,
  scrollRef,
  hotkeyTarget,
  onSelect,
  onDetails,
  onDelete,
  onNewSession,
}: SessionTreeProps) {
  const listRef = useRef<HTMLDivElement | null>(null);

  // Build a nested directory tree from session cwds — each path segment is a
  // collapsible dir node; sessions hang off the dir for their exact cwd.
  // Single-child dir chains fold GitHub-style (home/luis/GitHub → one node).
  // Once peers are registered the tree gains a node level so identical cwds
  // on different machines never merge into one folder.
  const multi = useMultiNode();
  const { dataMap, childrenMap, rootChildren, dirIds } = useMemo(
    () => treeFromSessions(sessions, { groupByNode: multi }),
    [sessions, multi],
  );

  // Controlled expandedItems: headless-tree's internal state and useTree's
  // injected React state drift apart (identical-object bail + stale merges
  // undo collapses). Owning expansion in React keeps them consistent.
  const [expanded, setExpanded] = useUiState<string[]>("ui.expandedDirs", []);
  const [, bumpRender] = useState(0);
  const tree = useTree<TreeData>({
    rootItemId: ROOT_ID,
    state: { expandedItems: expanded },
    setExpandedItems: setExpanded,
    // config.setState fires on every internal change; force a re-render for
    // non-controlled state (focus, selection). Deferred — setConfig can call
    // it mid-render.
    setState: () => {
      queueMicrotask(() => bumpRender((v) => v + 1));
    },
    getItemName: (item) => {
      const data = item.getItemData();
      if (data?.kind === "dir") return data.label;
      if (data?.kind === "node") return nodeName(data.node);
      if (data?.kind === "session") return data.session.title;
      return "sessions";
    },
    isItemFolder: (item) => {
      const kind = item.getItemData()?.kind;
      return kind === "dir" || kind === "node" || item.getId() === ROOT_ID;
    },
    dataLoader: {
      // Stale ids (filtered-out sessions still referenced by focus/expanded
      // state) hit getItem before the next rebuild — a throw crashes the
      // whole route, so return a transient empty dir instead.
      getItem: (id) =>
        (dataMap.get(id) as TreeData | undefined) ??
        ({ kind: "dir", label: "", cwd: "", count: 0 } as TreeData),
      getChildren: (id) => childrenMap.get(id) ?? [],
    },
    initialState: { expandedItems: rootChildren },
    features: [syncDataLoaderFeature],
    // Folders toggle via the item's built-in onClick — toggling here too
    // would double-collapse and re-expand. primaryAction is only for
    // the select behavior.
    onPrimaryAction: (item) => {
      const data = item.getItemData();
      if (data?.kind === "session") {
        onSelect(sessionKey(data.session));
      }
    },
    indent: 14,
  });

  // headless-tree only materializes items on an explicit rebuild — the
  // dataLoader alone doesn't trigger one.
  useEffect(() => {
    tree.rebuildTree();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessions, multi]);

  // Auto-expand only dirs that haven't been seen yet — a blanket merge here
  // re-ran whenever sessions changed identity and undid user collapses.
  const dirsKey = dirIds.join(",");
  const seenDirs = useRef<Set<string>>(new Set());
  useEffect(() => {
    const fresh = dirsKey.split(",").filter((id) => id !== "" && !seenDirs.current.has(id));
    seenDirs.current = new Set(dirsKey.split(",").filter(Boolean));
    if (fresh.length === 0) return;
    tree.applySubStateUpdate("expandedItems", (prev) => [...new Set([...(prev ?? []), ...fresh])]);
    tree.rebuildTree();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirsKey]);

  const items = tree.getItems();

  // The tree virtualizes against the sidebar's shared scroll element —
  // scrollMargin is where the list starts inside that scroller (sections sit
  // above), so scrollToIndex lands correctly.
  const [scrollMargin, setScrollMargin] = useState(0);
  useLayoutEffect(() => {
    const el = listRef.current;
    const scroller = scrollRef?.current;
    if (!el || !scroller) return;
    const margin =
      el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    if (margin !== scrollMargin) setScrollMargin(margin);
  });
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef?.current ?? null,
    estimateSize: () => 76,
    overscan: 8,
    scrollMargin,
  });

  const scrollToIndex = (index: number): void => {
    virtualizer.scrollToIndex(index, { align: "auto" });
  };

  // Keep the selected session's row visible (its group may be collapsed).
  useEffect(() => {
    if (selectedId === null) return;
    const index = items.findIndex((item) => item.getId() === `session:${selectedId}`);
    if (index !== -1) scrollToIndex(index);
    // items change every render; only re-scroll on selection change.
  }, [selectedId]); // eslint-disable-line react-hooks/exhaustive-deps

  const sessionRows = items.filter((item) => item.getItemData()?.kind === "session");
  const selectedIndex = sessionRows.findIndex(
    (item) =>
      sessionKey((item.getItemData() as { session: SessionSummary }).session) === selectedId,
  );

  const selectByIndex = (index: number): void => {
    const row = sessionRows[index];
    const data = row?.getItemData();
    if (data?.kind !== "session") return;
    const flatIndex = items.findIndex((item) => item.getId() === row.getId());
    if (flatIndex !== -1) scrollToIndex(flatIndex);
    onSelect(sessionKey(data.session));
  };

  // ignoreInputs: false so arrows still navigate while the filter input is focused.
  useAppHotkey(
    "nav.down",
    () => {
      const next = selectedIndex === -1 ? 0 : Math.min(selectedIndex + 1, sessionRows.length - 1);
      selectByIndex(next);
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );
  useAppHotkey(
    "nav.up",
    () => {
      const next = selectedIndex === -1 ? sessionRows.length - 1 : Math.max(selectedIndex - 1, 0);
      selectByIndex(next);
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );

  // Left/right collapse and expand the selected session's project group.
  const selectedGroup = (): ItemInstance<TreeData> | undefined => {
    const session = resolveSession(sessions, selectedId);
    if (session === undefined) return undefined;
    return tree.getItemInstance(sessionDirId(session.cwd, session.node, multi));
  };
  useAppHotkey(
    "nav.collapse",
    () => {
      const group = selectedGroup();
      if (group !== undefined && group.isExpanded()) group.collapse();
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );
  useAppHotkey(
    "nav.expand",
    () => {
      const group = selectedGroup();
      if (group !== undefined && !group.isExpanded()) group.expand();
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );

  return (
    <div className="p-2" ref={listRef}>
      <Tree tree={tree} indent={14}>
        <div className="relative w-full" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((row) => {
            const item = items[row.index];
            if (item === undefined) return null;
            const data = item.getItemData();
            return (
              <div
                key={item.getId()}
                data-index={row.index}
                ref={virtualizer.measureElement}
                className="group/row border-b border-border/50"
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${row.start - scrollMargin}px)`,
                }}
              >
                {data?.kind === "node" ? (
                  <NodeRow item={item} data={data} />
                ) : data?.kind === "dir" ? (
                  <GroupRow
                    item={item}
                    data={data}
                    sessions={sessions}
                    onNewSession={onNewSession}
                  />
                ) : data?.kind === "session" ? (
                  <SessionItemRow
                    item={item}
                    session={data.session}
                    selected={sessionKey(data.session) === selectedId}
                    onSelect={onSelect}
                    onDetails={onDetails}
                    onDelete={onDelete}
                  />
                ) : null}
              </div>
            );
          })}
        </div>
      </Tree>
    </div>
  );
}
