import { useEffect, useMemo, useRef, type RefObject } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useTree } from "@headless-tree/react";
import { syncDataLoaderFeature } from "@headless-tree/core";
import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area";
import type { ItemInstance } from "@headless-tree/core";
import type { SessionSummary } from "../../lib/types";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { ScrollBar } from "../ui/scroll-area";
import { Tree, TreeItem, TreeItemLabel } from "../reui/tree";

type TreeData =
  | { readonly kind: "group"; readonly label: string; readonly cwd: string; readonly count: number }
  | { readonly kind: "session"; readonly session: SessionSummary };

const ROOT_ID = "root";

function formatUpdated(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diffMinutes = Math.round((Date.now() - then) / 60000);
  if (diffMinutes < 1) return "just now";
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  const hours = Math.round(diffMinutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

const projectName = (cwd: string): string => {
  const trimmed = cwd.replace(/\/+$/, "");
  const last = trimmed.split("/").pop();
  return last === undefined || last === "" ? cwd : last;
};

function GroupRow({
  item,
  data,
}: {
  item: ItemInstance<TreeData>;
  data: TreeData & { kind: "group" };
}) {
  return (
    <TreeItem item={item} className="w-full">
      <TreeItemLabel>
        <span className="flex-1 truncate font-semibold" title={data.cwd}>
          {data.label}
        </span>
        <Badge variant="secondary">{data.count}</Badge>
      </TreeItemLabel>
    </TreeItem>
  );
}

function SessionItemRow({
  item,
  session,
  selected,
  onDelete,
}: {
  readonly item: ItemInstance<TreeData>;
  readonly session: SessionSummary;
  readonly selected: boolean;
  onDelete: (id: string) => void;
}) {
  return (
    <>
      <TreeItem
        item={item}
        data-selected={selected || undefined}
        className="w-full cursor-pointer border-0 bg-transparent p-0 text-left font-[inherit] text-inherit"
      >
        <TreeItemLabel className="w-full items-start in-data-[selected=true]:ring-1 in-data-[selected=true]:ring-inset in-data-[selected=true]:ring-primary">
          <div className="min-w-0 flex-1">
            <div className="flex items-center justify-between gap-2">
              <span className="truncate font-semibold">{session.title}</span>
              <Badge variant={session.agent === "cline" ? "outline" : "secondary"}>
                {session.agent}
              </Badge>
            </div>
            <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
              <span>{formatUpdated(session.updatedAt)}</span>
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
      <Button
        variant="ghost"
        size="icon-xs"
        className="absolute top-2 right-2 opacity-0 transition-opacity group-hover/row:opacity-100 hover:bg-secondary hover:text-destructive focus-visible:opacity-100"
        aria-label={`Delete session ${session.title}`}
        title="Delete session"
        onClick={(event) => {
          event.stopPropagation();
          if (window.confirm(`Delete session "${session.title}"?`)) {
            onDelete(session.id);
          }
        }}
      >
        ×
      </Button>
    </>
  );
}

interface SessionTreeProps {
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  readonly hotkeyTarget: RefObject<HTMLElement | null>;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
}

export function SessionTree({
  sessions,
  selectedId,
  hotkeyTarget,
  onSelect,
  onDelete,
}: SessionTreeProps) {
  const listRef = useRef<HTMLDivElement | null>(null);

  // Group sessions into project (cwd) folders for the tree. Groups order by
  // their most recently updated session.
  const { dataMap, childrenMap, rootChildren } = useMemo(() => {
    const data = new Map<string, TreeData>();
    const children = new Map<string, string[]>();
    const groups = new Map<string, SessionSummary[]>();
    for (const session of sessions) {
      const list = groups.get(session.cwd) ?? [];
      list.push(session);
      groups.set(session.cwd, list);
    }
    const ordered = [...groups.entries()].sort((a, b) =>
      (b[1][0]?.updatedAt ?? "").localeCompare(a[1][0]?.updatedAt ?? ""),
    );
    const rootChildren: string[] = [];
    for (const [cwd, items] of ordered) {
      const groupId = `group:${cwd}`;
      data.set(groupId, { kind: "group", label: projectName(cwd), cwd, count: items.length });
      children.set(
        groupId,
        items.map((session) => `session:${session.id}`),
      );
      for (const session of items) {
        data.set(`session:${session.id}`, { kind: "session", session });
      }
      rootChildren.push(groupId);
    }
    children.set(ROOT_ID, rootChildren);
    return { dataMap: data, childrenMap: children, rootChildren };
  }, [sessions]);

  const tree = useTree<TreeData>({
    rootItemId: ROOT_ID,
    getItemName: (item) => {
      const data = item.getItemData();
      if (data?.kind === "group") return data.label;
      if (data?.kind === "session") return data.session.title;
      return "sessions";
    },
    isItemFolder: (item) => item.getItemData()?.kind === "group" || item.getId() === ROOT_ID,
    dataLoader: {
      getItem: (id) => dataMap.get(id) as TreeData,
      getChildren: (id) => childrenMap.get(id) ?? [],
    },
    initialState: { expandedItems: rootChildren },
    features: [syncDataLoaderFeature],
    onPrimaryAction: (item) => {
      const data = item.getItemData();
      if (data?.kind === "session") {
        onSelect(data.session.id);
      } else if (item.isExpanded()) {
        item.collapse();
      } else {
        item.expand();
      }
    },
    indent: 14,
  });

  // Auto-expand groups that appear (new sessions, filter hits) without
  // disturbing groups the user collapsed manually.
  const groupsKey = rootChildren.join(",");
  useEffect(() => {
    tree.applySubStateUpdate("expandedItems", (prev) => [
      ...new Set([...(prev ?? []), ...groupsKey.split(",").filter(Boolean)]),
    ]);
  }, [groupsKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const items = tree.getItems();

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 76,
    overscan: 8,
  });

  // Keep the selected session's row visible (its group may be collapsed).
  useEffect(() => {
    if (selectedId === null) return;
    const index = items.findIndex((item) => item.getId() === `session:${selectedId}`);
    if (index !== -1) virtualizer.scrollToIndex(index, { align: "auto" });
    // items/virtualizer change every render; only re-scroll on selection change.
  }, [selectedId]); // eslint-disable-line react-hooks/exhaustive-deps

  const sessionRows = items.filter((item) => item.getItemData()?.kind === "session");
  const selectedIndex = sessionRows.findIndex(
    (item) => (item.getItemData() as { session: SessionSummary }).session.id === selectedId,
  );

  const selectByIndex = (index: number): void => {
    const row = sessionRows[index];
    const data = row?.getItemData();
    if (data?.kind !== "session") return;
    const flatIndex = items.findIndex((item) => item.getId() === row.getId());
    if (flatIndex !== -1) virtualizer.scrollToIndex(flatIndex, { align: "auto" });
    onSelect(data.session.id);
  };

  // ignoreInputs: false so arrows still navigate while the filter input is focused.
  useHotkey(
    "ArrowDown",
    () => {
      const next = selectedIndex === -1 ? 0 : Math.min(selectedIndex + 1, sessionRows.length - 1);
      selectByIndex(next);
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "ArrowUp",
    () => {
      const next = selectedIndex === -1 ? sessionRows.length - 1 : Math.max(selectedIndex - 1, 0);
      selectByIndex(next);
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );

  // Left/right collapse and expand the selected session's project group.
  const selectedGroup = (): ItemInstance<TreeData> | undefined => {
    const session = sessions.find((s) => s.id === selectedId);
    if (session === undefined) return undefined;
    return tree.getItemInstance(`group:${session.cwd}`);
  };
  useHotkey(
    "ArrowLeft",
    () => {
      const group = selectedGroup();
      if (group !== undefined && group.isExpanded()) group.collapse();
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "ArrowRight",
    () => {
      const group = selectedGroup();
      if (group !== undefined && !group.isExpanded()) group.expand();
    },
    { target: hotkeyTarget, preventDefault: true, ignoreInputs: false },
  );

  return (
    <ScrollAreaPrimitive.Root className="flex min-h-0 flex-1 flex-col">
      <ScrollAreaPrimitive.Viewport className="h-full p-2" ref={listRef}>
        <Tree tree={tree} indent={14}>
          <div className="relative" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            {virtualizer.getVirtualItems().map((row) => {
              const item = items[row.index];
              if (item === undefined) return null;
              const data = item.getItemData();
              return (
                <div
                  key={item.getId()}
                  data-index={row.index}
                  ref={virtualizer.measureElement}
                  className="group/row"
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${row.start}px)`,
                  }}
                >
                  {data?.kind === "group" ? (
                    <GroupRow item={item} data={data} />
                  ) : data?.kind === "session" ? (
                    <SessionItemRow
                      item={item}
                      session={data.session}
                      selected={data.session.id === selectedId}
                      onDelete={onDelete}
                    />
                  ) : null}
                </div>
              );
            })}
          </div>
        </Tree>
      </ScrollAreaPrimitive.Viewport>
      <ScrollBar />
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  );
}
