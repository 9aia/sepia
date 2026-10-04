import { useEffect, useMemo, useRef, useState } from "react";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useStore } from "@tanstack/react-store";
import { AlertCircleIcon, FolderOpenIcon, SearchAreaIcon } from "@hugeicons/core-free-icons";
import { sepiaStore, setSelectedId } from "../lib/store";
import type { SessionSummary } from "../lib/types";
import { useAgents } from "../hooks/query/useAgents";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useSessions } from "../hooks/query/useSessions";
import { Sidebar } from "./ui/sidebar";
import { Spinner } from "./ui/spinner";
import { EmptyScreen } from "./EmptyScreen";
import { CreateForm } from "./session-list/CreateForm";
import {
  FilterBar,
  type DateFilter,
  type SortKey,
  type StatusFilter,
} from "./session-list/FilterBar";
import { settingsStore } from "../lib/settings";
import { SessionTree } from "./session-list/SessionTree";
import { SessionDetailsDrawer } from "./session-list/SessionDetailsDrawer";
import { UserProfile } from "./session-list/UserProfile";

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

export function SessionList() {
  const { data: sessions = [], isLoading: loading, error } = useSessions();
  const { data: agents = [] } = useAgents();
  const deleteMutation = useDeleteSession();
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);
  const settings = useStore(settingsStore);
  const [filter, setFilter] = useState("");
  const [agentFilter, setAgentFilter] = useState<string[]>([]);
  const [dateFilter, setDateFilter] = useState<DateFilter>("all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [sort, setSort] = useState<SortKey>("newest");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [details, setDetails] = useState<{ id: string; rename: boolean } | null>(null);
  const [modKey, setModKey] = useState("Ctrl");
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);
  const [debouncedFilter] = useDebouncedValue(filter, { wait: 200 });
  const asideRef = useRef<HTMLDivElement | null>(null);
  const filterRef = useRef<HTMLInputElement | null>(null);
  const cwdRef = useRef<HTMLInputElement | null>(null);

  const filtered = useMemo(() => {
    const needle = debouncedFilter.trim().toLowerCase();
    const cutoff = dateFilter === "all" ? null : DATE_CUTOFFS[dateFilter];
    const now = Date.now();
    return sessions
      .filter((session) => {
        if (agentFilter.length > 0 && !agentFilter.includes(session.agent)) return false;
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

  useHotkey("Mod+K", () => filterRef.current?.focus(), { preventDefault: true });
  useHotkey("N", () => {
    if (inFormField()) return;
    setAdvancedOpen(true);
    requestAnimationFrame(() => cwdRef.current?.focus());
  });
  useHotkey(
    "Escape",
    () => {
      setFilter("");
      filterRef.current?.blur();
    },
    { target: filterRef },
  );

  return (
    <Sidebar collapsible="offcanvas" ref={asideRef}>
      <div className="flex flex-row items-center justify-between border-b border-border p-4">
        <h1 className="m-0 text-base tracking-wide">sepia</h1>
        <span className="text-muted-foreground tabular-nums">{filtered.length}</span>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <CreateForm
          cwdRef={cwdRef}
          agents={agents}
          defaultCwd={settings.defaultCwd ?? sessions[0]?.cwd}
          defaultAgent={settings.defaultAgent}
          open={advancedOpen}
          onOpenChange={setAdvancedOpen}
        />
        <FilterBar
          agents={agents}
          filter={filter}
          filterRef={filterRef}
          modKey={modKey}
          agentFilter={agentFilter}
          dateFilter={dateFilter}
          statusFilter={statusFilter}
          sort={sort}
          onFilterChange={setFilter}
          onToggleAgent={(id, checked) =>
            setAgentFilter((prev) =>
              checked ? [...prev, id] : prev.filter((agent) => agent !== id),
            )
          }
          onDateFilterChange={setDateFilter}
          onStatusFilterChange={setStatusFilter}
          onSortChange={setSort}
        />

        {loading && (
          <div className="flex items-center gap-2 p-4 text-muted-foreground">
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

        <SessionTree
          sessions={filtered}
          selectedId={selectedId}
          hotkeyTarget={asideRef}
          onSelect={setSelectedId}
          onDetails={(id, rename) => setDetails({ id, rename })}
          onDelete={(id) => deleteMutation.mutate(id)}
        />
      </div>

      <SessionDetailsDrawer
        session={sessions.find((s) => s.id === details?.id)}
        focusRename={details?.rename ?? false}
        onClose={() => setDetails(null)}
      />

      <UserProfile />
    </Sidebar>
  );
}
