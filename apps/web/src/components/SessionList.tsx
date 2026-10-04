import { useMemo, useRef, useState, type FormEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useStore } from "@tanstack/react-store";
import { sepiaStore, setSelectedId } from "../lib/store";
import type { SessionSummary } from "../lib/types";
import { useAgents } from "../hooks/query/useAgents";
import { useCreateSession } from "../hooks/query/useCreateSession";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useSessions } from "../hooks/query/useSessions";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

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

  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 76,
    overscan: 8,
  });

  const selectedIndex = filtered.findIndex((session) => session.id === selectedId);

  const selectByIndex = (index: number): void => {
    const session = filtered[index];
    if (session === undefined) return;
    virtualizer.scrollToIndex(index, { align: "auto" });
    setSelectedId(session.id);
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
      const next = selectedIndex === -1 ? 0 : Math.min(selectedIndex + 1, filtered.length - 1);
      selectByIndex(next);
    },
    { target: asideRef, preventDefault: true, ignoreInputs: false },
  );
  useHotkey(
    "ArrowUp",
    () => {
      const next = selectedIndex === -1 ? filtered.length - 1 : Math.max(selectedIndex - 1, 0);
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

      <Input
        type="search"
        className="session-list__search"
        placeholder="Filter sessions… (Ctrl/⌘+K)"
        aria-label="Filter sessions"
        ref={filterRef}
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />

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

      {loading && <p className="session-list__status">Loading sessions…</p>}
      {error !== null && (
        <p className="session-list__status session-list__status--error">
          {messageOf(error, "Failed to list sessions")}
        </p>
      )}
      {!loading && !error && filtered.length === 0 && (
        <p className="session-list__status">
          {sessions.length === 0 ? "No sessions found." : "No sessions match the filter."}
        </p>
      )}

      <div
        className="session-list__items"
        ref={listRef}
        role="listbox"
        aria-label="Sessions"
        aria-activedescendant={selectedId === null ? undefined : `session-option-${selectedId}`}
      >
        <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const session = filtered[row.index];
            if (session === undefined) return null;
            const selected = session.id === selectedId;
            return (
              <div
                key={session.id}
                id={`session-option-${session.id}`}
                role="option"
                aria-selected={selected}
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
                <button
                  type="button"
                  className={"session-item" + (selected ? " session-item--selected" : "")}
                  onClick={() => setSelectedId(session.id)}
                >
                  <div className="session-item__top">
                    <span className="session-item__title">{session.title}</span>
                    <Badge variant={session.agent === "cline" ? "outline" : "secondary"}>
                      {session.agent}
                    </Badge>
                  </div>
                  <div className="session-item__cwd" title={session.cwd}>
                    {session.cwd}
                  </div>
                  <div className="session-item__meta">
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
                </button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="session-item__delete"
                  aria-label={`Delete session ${session.title}`}
                  title="Delete session"
                  onClick={(event) => {
                    event.stopPropagation();
                    if (window.confirm(`Delete session "${session.title}"?`)) {
                      deleteMutation.mutate(session.id);
                    }
                  }}
                >
                  ×
                </Button>
              </div>
            );
          })}
        </div>
      </div>
    </aside>
  );
}
