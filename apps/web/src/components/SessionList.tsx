import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useStore } from "@tanstack/react-store";
import { useTree } from "@headless-tree/react";
import { syncDataLoaderFeature } from "@headless-tree/core";
import { sepiaStore, setSelectedId } from "../lib/store";
import { AlertCircleIcon, FolderOpenIcon, SearchAreaIcon } from "@hugeicons/core-free-icons";
import type { SessionSummary } from "../lib/types";
import { useAgents } from "../hooks/query/useAgents";
import { useCreateSession } from "../hooks/query/useCreateSession";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useSessions } from "../hooks/query/useSessions";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";
import { Kbd, KbdGroup } from "./ui/kbd";
import { Spinner } from "./ui/spinner";
import { EmptyScreen } from "./EmptyScreen";
import { Tree, TreeItem, TreeItemLabel } from "./reui/tree";

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

const inFormField = (): boolean => {
  const el = document.activeElement;
  return (
    el instanceof HTMLInputElement ||
    el instanceof HTMLTextAreaElement ||
    el instanceof HTMLSelectElement
  );
};

const messageOf = (err: unknown, fallback: string): string =>
  err instanceof Error ? err.message : fallback;

type DateFilter = "all" | "day" | "week" | "month";
type StatusFilter = "all" | "free" | "locked";
type SortKey = "newest" | "oldest" | "title";

const DATE_CUTOFFS: Record<Exclude<DateFilter, "all">, number> = {
  day: 24 * 60 * 60 * 1000,
  week: 7 * 24 * 60 * 60 * 1000,
  month: 30 * 24 * 60 * 60 * 1000,
};

const sorters: Record<SortKey, (a: SessionSummary, b: SessionSummary) => number> = {
  newest: (a, b) => b.updatedAt.localeCompare(a.updatedAt),
  oldest: (a, b) => a.updatedAt.localeCompare(b.updatedAt),
  title: (a, b) => a.title.localeCompare(b.title),
};

type TreeData =
  | { readonly kind: "group"; readonly label: string; readonly cwd: string; readonly count: number }
  | { readonly kind: "session"; readonly session: SessionSummary };

const projectName = (cwd: string): string => {
  const trimmed = cwd.replace(/\/+$/, "");
  const last = trimmed.split("/").pop();
  return last === undefined || last === "" ? cwd : last;
};

const ROOT_ID = "root";

export function SessionList() {
  const { data: sessions = [], isLoading: loading, error } = useSessions();
  const { data: agents = [] } = useAgents();
  const createMutation = useCreateSession();
  const deleteMutation = useDeleteSession();
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);
  const creating = createMutation.isPending;
  const createError = createMutation.isError
    ? messageOf(createMutation.error, "Failed to create session")
    : null;
  const [cwd, setCwd] = useState("");
  const [title, setTitle] = useState("");
  const [agent, setAgent] = useState("devin");
  const [filter, setFilter] = useState("");
  const [agentFilter, setAgentFilter] = useState<string>("all");
  const [dateFilter, setDateFilter] = useState<DateFilter>("all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [sort, setSort] = useState<SortKey>("newest");
  const [modKey, setModKey] = useState("Ctrl");
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);
  const [debouncedFilter] = useDebouncedValue(filter, { wait: 200 });
  const asideRef = useRef<HTMLElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const filterRef = useRef<HTMLInputElement | null>(null);
  const cwdRef = useRef<HTMLInputElement | null>(null);

  const filtered = useMemo(() => {
    const needle = debouncedFilter.trim().toLowerCase();
    const cutoff = dateFilter === "all" ? null : DATE_CUTOFFS[dateFilter];
    const now = Date.now();
    return sessions
      .filter((session) => {
        if (agentFilter !== "all" && session.agent !== agentFilter) return false;
        if (statusFilter === "locked" && !session.locked) return false;
        if (statusFilter === "free" && session.locked) return false;
        if (cutoff !== null && now - new Date(session.updatedAt).getTime() > cutoff) return false;
        return (
          needle === "" ||
          [session.title, session.cwd, session.id].some((field) =>
            field.toLowerCase().includes(needle),
          )
        );
      })
      .sort(sorters[sort]);
  }, [sessions, debouncedFilter, agentFilter, dateFilter, statusFilter, sort]);

  // Group filtered sessions into project (cwd) folders for the tree. Groups
  // order by their most recently updated session.
  const { dataMap, childrenMap, rootChildren } = useMemo(() => {
    const data = new Map<string, TreeData>();
    const children = new Map<string, string[]>();
    const groups = new Map<string, SessionSummary[]>();
    for (const session of filtered) {
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
  }, [filtered]);

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
        setSelectedId(data.session.id);
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
    setSelectedId(data.session.id);
  };

  useHotkey("Mod+K", () => filterRef.current?.focus(), { preventDefault: true });
  useHotkey("N", () => {
    if (inFormField()) return;
    cwdRef.current?.focus();
  });
  // ignoreInputs: false so arrows still navigate while the filter input is focused.
  useHotkey(
    "ArrowDown",
    () => {
      const next = selectedIndex === -1 ? 0 : Math.min(selectedIndex + 1, sessionRows.length - 1);
      selectByIndex(next);
    },
    { target: asideRef, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "ArrowUp",
    () => {
      const next = selectedIndex === -1 ? sessionRows.length - 1 : Math.max(selectedIndex - 1, 0);
      selectByIndex(next);
    },
    { target: asideRef, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "Escape",
    () => {
      setFilter("");
      filterRef.current?.blur();
    },
    { target: filterRef },
  );

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const trimmedCwd = cwd.trim();
    if (trimmedCwd === "") return;
    const trimmedTitle = title.trim();
    createMutation.mutate({
      cwd: trimmedCwd,
      agent,
      title: trimmedTitle === "" ? undefined : trimmedTitle,
    });
    setCwd("");
    setTitle("");
  };

  return (
    <aside className="session-list" ref={asideRef}>
      <header className="session-list__header">
        <h1 className="session-list__title">sepia</h1>
        <span className="session-list__count">{filtered.length}</span>
      </header>

      <form className="session-list__new" onSubmit={submit}>
        <Input
          type="text"
          placeholder="Working directory (absolute)"
          aria-label="Working directory"
          ref={cwdRef}
          value={cwd}
          onChange={(event) => setCwd(event.target.value)}
        />
        <Input
          type="text"
          placeholder="Title (optional)"
          aria-label="Session title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        {agents.length > 1 && (
          <Select
            value={agent}
            onValueChange={(value) => {
              if (value !== null) setAgent(value);
            }}
          >
            <SelectTrigger aria-label="Agent">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {agents.map((a) => (
                <SelectItem key={a.id} value={a.id}>
                  {a.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Button type="submit" disabled={creating || cwd.trim() === ""}>
          {creating ? "Creating…" : "New session"}
        </Button>
        {createError && (
          <p className="session-list__status session-list__status--error">{createError}</p>
        )}
      </form>

      <div className="session-list__search">
        <Input
          type="search"
          className="pr-16"
          placeholder="Filter sessions…"
          aria-label="Filter sessions"
          ref={filterRef}
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <KbdGroup className="session-list__search-kbd" aria-hidden="true">
          <Kbd>{modKey}</Kbd>
          <Kbd>K</Kbd>
        </KbdGroup>
      </div>

      <div className="session-list__filters">
        <Select
          value={agentFilter}
          onValueChange={(value) => {
            if (value !== null) setAgentFilter(value);
          }}
        >
          <SelectTrigger aria-label="Filter by agent" className="h-7 flex-1 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All agents</SelectItem>
            {agents.map((a) => (
              <SelectItem key={a.id} value={a.id}>
                {a.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={dateFilter}
          onValueChange={(value) => {
            if (value !== null) setDateFilter(value as DateFilter);
          }}
        >
          <SelectTrigger aria-label="Filter by recency" className="h-7 flex-1 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Any time</SelectItem>
            <SelectItem value="day">Today</SelectItem>
            <SelectItem value="week">Last 7 days</SelectItem>
            <SelectItem value="month">Last 30 days</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={statusFilter}
          onValueChange={(value) => {
            if (value !== null) setStatusFilter(value as StatusFilter);
          }}
        >
          <SelectTrigger aria-label="Filter by lock status" className="h-7 flex-1 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">Any status</SelectItem>
            <SelectItem value="free">Free</SelectItem>
            <SelectItem value="locked">Locked</SelectItem>
          </SelectContent>
        </Select>
        <Select
          value={sort}
          onValueChange={(value) => {
            if (value !== null) setSort(value as SortKey);
          }}
        >
          <SelectTrigger aria-label="Sort sessions" className="h-7 flex-1 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="newest">Newest</SelectItem>
            <SelectItem value="oldest">Oldest</SelectItem>
            <SelectItem value="title">Title</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {loading && (
        <div className="session-list__status">
          <Spinner /> Loading sessions…
        </div>
      )}
      {error !== null && (
        <EmptyScreen
          className="p-6"
          icon={AlertCircleIcon}
          title="Couldn't load sessions"
          description={messageOf(error, "Failed to list sessions")}
        />
      )}
      {!loading && !error && filtered.length === 0 && sessions.length === 0 && (
        <EmptyScreen
          className="p-6"
          icon={FolderOpenIcon}
          title="No sessions yet"
          description="Create your first session above."
        />
      )}
      {!loading && !error && filtered.length === 0 && sessions.length > 0 && (
        <EmptyScreen
          className="p-6"
          icon={SearchAreaIcon}
          title="No matches"
          description="No sessions match the current filters."
        />
      )}

      <div className="session-list__items" ref={listRef}>
        <Tree tree={tree} indent={14} className="session-tree">
          <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
            {virtualizer.getVirtualItems().map((row) => {
              const item = items[row.index];
              if (item === undefined) return null;
              const data = item.getItemData();
              return (
                <div
                  key={item.getId()}
                  data-index={row.index}
                  ref={virtualizer.measureElement}
                  className="session-row"
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${row.start}px)`,
                  }}
                >
                  {data?.kind === "group" ? (
                    <TreeItem item={item} className="session-group">
                      <TreeItemLabel>
                        <span className="session-group__label" title={data.cwd}>
                          {data.label}
                        </span>
                        <Badge variant="secondary">{data.count}</Badge>
                      </TreeItemLabel>
                    </TreeItem>
                  ) : data?.kind === "session" ? (
                    <>
                      <TreeItem
                        item={item}
                        className={
                          "session-item" +
                          (data.session.id === selectedId ? " session-item--selected" : "")
                        }
                      >
                        <TreeItemLabel className="session-item__label">
                          <div className="session-item__body">
                            <div className="session-item__top">
                              <span className="session-item__title">{data.session.title}</span>
                              <Badge
                                variant={data.session.agent === "cline" ? "outline" : "secondary"}
                              >
                                {data.session.agent}
                              </Badge>
                            </div>
                            <div className="session-item__meta">
                              <span>{formatUpdated(data.session.updatedAt)}</span>
                              {data.session.locked && (
                                <Badge
                                  variant="destructive"
                                  title={`Locked by pid ${data.session.lockHolderPid ?? "unknown"}`}
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
                        className="session-item__delete"
                        aria-label={`Delete session ${data.session.title}`}
                        title="Delete session"
                        onClick={(event) => {
                          event.stopPropagation();
                          if (window.confirm(`Delete session "${data.session.title}"?`)) {
                            deleteMutation.mutate(data.session.id);
                          }
                        }}
                      >
                        ×
                      </Button>
                    </>
                  ) : null}
                </div>
              );
            })}
          </div>
        </Tree>
      </div>

      <footer className="session-list__shortcuts">
        <span>
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd> navigate
        </span>
        <span>
          <Kbd>N</Kbd> new session
        </span>
        <span>
          <Kbd>Esc</Kbd> clear filter
        </span>
      </footer>
    </aside>
  );
}
