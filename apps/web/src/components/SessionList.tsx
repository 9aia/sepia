import { useEffect, useMemo, useRef, useState } from "react";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useStore } from "@tanstack/react-store";
import { AlertCircleIcon, FolderOpenIcon, SearchAreaIcon } from "@hugeicons/core-free-icons";
import { sepiaStore, setCreateCwd, setCwd, setDetailsFor, setSelectedId } from "../lib/store";
import { useNavigate, useSearch } from "@tanstack/react-router";
import type { SessionSummary } from "../lib/types";
import { useAgents } from "../hooks/query/useAgents";
import { useCreateSession } from "../hooks/query/useCreateSession";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useUserInfo } from "../hooks/query/useUserInfo";
import { modelArgsFor } from "../lib/models";
import { useSessions } from "../hooks/query/useSessions";
import { Button } from "./ui/button";
import { Sidebar } from "./ui/sidebar";
import { EmptyScreen } from "./EmptyScreen";
import { CwdPicker } from "./session-list/CwdPicker";
import {
  FilterBar,
  type DateFilter,
  type SortKey,
  type StatusFilter,
} from "./session-list/FilterBar";
import { settingsStore } from "../lib/settings";
import { SessionTree } from "./session-list/SessionTree";
import { SessionTreeSkeleton } from "./session-list/SessionTreeSkeleton";
import { SessionDetailsDrawer } from "./session-list/SessionDetailsDrawer";
import { SessionSections } from "./session-list/SessionSections";
import { getRecents } from "../lib/recents";
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
  const navigate = useNavigate({ from: "/" });
  const search = useSearch({ from: "/" });
  const patchSearch = (update: Record<string, string | undefined>): void => {
    void navigate({ search: (prev) => ({ ...prev, ...update }), replace: true });
  };
  const filter = search.q ?? "";
  const agentFilter = search.agents?.split(",").filter((a) => a !== "") ?? [];
  const dateFilter = (search.date as DateFilter | undefined) ?? "all";
  const statusFilter = (search.status as StatusFilter | undefined) ?? "all";
  const sort = (search.sort as SortKey | undefined) ?? "newest";
  const createMutation = useCreateSession();
  const details = useStore(sepiaStore, (state) => state.detailsFor);
  const [modKey, setModKey] = useState("Ctrl");
  const createCwd = useStore(sepiaStore, (state) => state.createCwd);
  const cwd = useStore(sepiaStore, (state) => state.cwd);
  const { data: user } = useUserInfo();
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);
  // The dir new sessions spawn in: explicit pick > settings default >
  // most recent session's > home.
  const resolvedCwd = cwd ?? settings.defaultCwd ?? sessions[0]?.cwd ?? user?.homedir ?? "/";
  const create = (dir: string): void => {
    const agent = settings.defaultAgent ?? agents[0]?.id;
    createMutation.mutate({
      cwd: dir,
      agent,
      ...modelArgsFor(agent ?? "", null, settings),
    });
  };
  // "New session here" from a dir row → set the context dir and create.
  useEffect(() => {
    if (createCwd === null) return;
    setCreateCwd(null);
    setCwd(createCwd);
    create(createCwd);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [createCwd]);
  const [debouncedFilter] = useDebouncedValue(filter, { wait: 200 });
  const asideRef = useRef<HTMLDivElement | null>(null);
  const filterRef = useRef<HTMLInputElement | null>(null);

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

  const recentSessions = useMemo(() => {
    const byId = new Map(filtered.map((s) => [s.id, s]));
    return getRecents()
      .map((id) => byId.get(id))
      .filter((s): s is NonNullable<typeof s> => s !== undefined);
    // selectedId change refreshes the MRU
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filtered, selectedId]);

  useHotkey("N", () => {
    if (inFormField()) return;
    create(resolvedCwd);
  });
  useHotkey(
    "Escape",
    () => {
      patchSearch({ q: undefined });
      filterRef.current?.blur();
    },
    { target: filterRef },
  );

  return (
    <Sidebar collapsible="offcanvas" ref={asideRef}>
      <div className="flex flex-row items-center gap-2.5 border-b border-border p-4">
        <span
          aria-hidden="true"
          className="flex size-6 items-center justify-center rounded-md bg-primary text-xs font-bold text-primary-foreground select-none"
        >
          S
        </span>
        <h1 className="m-0 text-base tracking-wide">Sepia</h1>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div className="flex flex-col gap-1.5 border-b border-border px-4 py-3">
          <Button onClick={() => create(resolvedCwd)} disabled={createMutation.isPending}>
            {createMutation.isPending ? "Creating…" : "New session"}
          </Button>
          <CwdPicker
            value={resolvedCwd}
            dirs={[...new Set(sessions.map((s) => s.cwd))]}
            onChange={setCwd}
          />
        </div>
        <FilterBar
          agents={agents}
          filter={filter}
          filterRef={filterRef}
          modKey={modKey}
          agentFilter={agentFilter}
          dateFilter={dateFilter}
          statusFilter={statusFilter}
          sort={sort}
          onFilterChange={(q) => patchSearch({ q: q === "" ? undefined : q })}
          onToggleAgent={(id, checked) => {
            const next = checked
              ? [...agentFilter, id]
              : agentFilter.filter((agent) => agent !== id);
            patchSearch({ agents: next.length === 0 ? undefined : next.join(",") });
          }}
          onDateFilterChange={(value) => patchSearch({ date: value === "all" ? undefined : value })}
          onStatusFilterChange={(value) =>
            patchSearch({ status: value === "all" ? undefined : value })
          }
          onSortChange={(value) => patchSearch({ sort: value === "newest" ? undefined : value })}
        />

        <SessionSections
          sessions={filtered}
          recentSessions={recentSessions}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onDetails={(id, rename) => setDetailsFor({ id, rename })}
          onDelete={(id) => deleteMutation.mutate(id)}
        />

        {loading && <SessionTreeSkeleton />}
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
          onDetails={(id, rename) => setDetailsFor({ id, rename })}
          onDelete={(id) => deleteMutation.mutate(id)}
          onNewSession={setCreateCwd}
        />
      </div>

      <SessionDetailsDrawer
        session={sessions.find((s) => s.id === details?.id)}
        focusRename={details?.rename ?? false}
        onClose={() => setDetailsFor(null)}
        onOpen={setSelectedId}
        onDelete={(id) => deleteMutation.mutate(id)}
      />

      <UserProfile />
    </Sidebar>
  );
}
