import { useEffect, useMemo, useRef, useState } from "react";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { useAppHotkey } from "../lib/keybinds";
import { isFormField } from "../lib/keyboard";
import { useStore } from "@tanstack/react-store";
import {
  AlertCircleIcon,
  ChevronDownIcon,
  PlusSignIcon,
  SearchAreaIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  sepiaStore,
  setCreateCwd,
  setCwd,
  setDetailsFor,
  setNewProjectFor,
  setSelectedId,
} from "../lib/store";
import { useNavigate, useSearch } from "@tanstack/react-router";
import type { SessionSummary } from "../lib/types";
import { useAgents } from "../hooks/query/useAgents";
import { useCreateSession } from "../hooks/query/useCreateSession";
import { Skeleton } from "./ui/skeleton";
import { usePatchSessionMeta } from "../hooks/query/useSessionMeta";
import { useCreateProject } from "../hooks/query/useProjects";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useUserInfo } from "../hooks/query/useUserInfo";
import { modelArgsFor } from "../lib/models";
import { bareProjectId, isLocalNode, projectKey, resolveSession, sessionKey } from "../lib/format";
import { useSessions } from "../hooks/query/useSessions";
import { Button } from "./ui/button";
import { ButtonGroup } from "./ui/button-group";
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from "./ui/dropdown-menu";
import { ScrollArea } from "./ui/scroll-area";
import { Sidebar, useSidebar } from "./ui/sidebar";
import { EmptyScreen } from "./EmptyScreen";
import { CwdPicker } from "./session-list/CwdPicker";
import {
  FilterBar,
  type DateFilter,
  type SortKey,
  type StatusFilter,
} from "./session-list/FilterBar";
import { defaultAgentFor, defaultCwdFor, recentCwdFor, settingsStore } from "../lib/settings";
import { SessionTreeSkeleton } from "./session-list/SessionTreeSkeleton";
import { ListEmptyState } from "./session-list/ListEmptyState";
import { ProjectNameDialog, SessionSections } from "./session-list/SessionSections";
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

const inFormField = (): boolean => isFormField(document.activeElement);

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
  const [modKey, setModKey] = useState("Ctrl");
  const newProjectFor = useStore(sepiaStore, (state) => state.newProjectFor);
  const createProject = useCreateProject();
  const patch = usePatchSessionMeta();
  const newProjectSession = resolveSession(sessions, newProjectFor);
  // Ids collide across agents; pass the agent so the delete hits the right store.
  const onDeleteSession = (session: SessionSummary): void => {
    deleteMutation.mutate({ id: session.id, agent: session.agent });
  };
  const createCwd = useStore(sepiaStore, (state) => state.createCwd);
  const createNode = useStore(sepiaStore, (state) => state.createNode);
  const cwd = useStore(sepiaStore, (state) => state.cwd);
  const { data: user } = useUserInfo();
  const { isMobile, setOpenMobile } = useSidebar();
  const selectAndClose = (key: string): void => {
    setSelectedId(key);
    if (isMobile) setOpenMobile(false);
  };
  const detailsAndClose = (id: string, rename: boolean): void => {
    setDetailsFor({ id, rename });
    if (isMobile) setOpenMobile(false);
  };
  useEffect(() => {
    if (navigator.platform.toUpperCase().includes("MAC")) setModKey("⌘");
  }, []);
  // The dir new sessions spawn in — the button targets the local node, so
  // the chain resolves locally: explicit pick > this node's settings
  // default > most recent local session's > home.
  const resolvedCwd =
    cwd ??
    defaultCwdFor(settings, undefined) ??
    recentCwdFor(sessions, undefined) ??
    user?.homedir ??
    "/";
  const create = (dir: string, node?: string): void => {
    if (isMobile) setOpenMobile(false);
    // The configured default is scoped to the target node; when it's unset
    // the local node keeps the roster's first agent, while a peer gets no
    // override and picks its own default (a local-only id would just fail).
    const agent =
      defaultAgentFor(settings, node) ?? (isLocalNode(node) ? agents[0]?.id : undefined);
    createMutation.mutate({
      cwd: dir,
      agent,
      node,
      ...modelArgsFor(agent ?? "", null, settings),
    });
  };
  // "New session here" from a dir row → set the context dir and create on
  // the folder's node (a peer path isn't a usable local default cwd).
  useEffect(() => {
    if (createCwd === null) return;
    const node = createNode ?? undefined;
    setCreateCwd(null);
    if (isLocalNode(node)) setCwd(createCwd);
    create(createCwd, node);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [createCwd]);
  const [debouncedFilter] = useDebouncedValue(filter, { wait: 200 });
  const asideRef = useRef<HTMLDivElement | null>(null);
  const bodyScrollRef = useRef<HTMLDivElement | null>(null);
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

  // Archived sessions leave every normal view — they only surface in the
  // Archived section (still searchable, so unarchiving is findable).
  const activeSessions = useMemo(() => filtered.filter((s) => s.archived !== true), [filtered]);
  const archivedSessions = useMemo(() => filtered.filter((s) => s.archived === true), [filtered]);

  const recentSessions = useMemo(() => {
    // Recents may hold the same session twice — bare-id entries from before
    // the agent-scoped keys, plus the scoped one. Resolve then dedup by key.
    const seen = new Set<string>();
    return getRecents()
      .map((key) => resolveSession(activeSessions, key))
      .filter((s): s is NonNullable<typeof s> => {
        if (s === undefined) return false;
        const key = sessionKey(s);
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      });
    // selectedId change refreshes the MRU
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSessions, selectedId]);

  // Bare N — inFormField guards against typing; Mod+N is browser-reserved
  // (Ctrl+N = new window can't be preventDefault'd in Chrome/Firefox).
  useAppHotkey("session.new", () => {
    if (inFormField()) return;
    create(resolvedCwd);
  });
  useAppHotkey(
    "filter.clear",
    () => {
      patchSearch({ q: undefined });
      filterRef.current?.blur();
    },
    { target: filterRef },
  );

  return (
    <Sidebar collapsible="offcanvas" ref={asideRef}>
      <div className="flex flex-row items-center gap-2.5 px-4 pt-[max(0.75rem,env(safe-area-inset-top))] pb-3">
        <h1 className="m-0 text-base font-medium tracking-wide">Sepia</h1>
        <div className="ml-auto flex items-center gap-1">
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
            onDateFilterChange={(value) =>
              patchSearch({ date: value === "all" ? undefined : value })
            }
            onStatusFilterChange={(value) =>
              patchSearch({ status: value === "all" ? undefined : value })
            }
            onSortChange={(value) => patchSearch({ sort: value === "newest" ? undefined : value })}
          />
        </div>
      </div>

      <div className="flex shrink-0 flex-col gap-1.5 px-4 py-3">
        {loading ? (
          <>
            <Skeleton className="h-9 w-full rounded-4xl" />
            <Skeleton className="h-8 w-full rounded-3xl" />
          </>
        ) : (
          <>
            <ButtonGroup className="w-full">
              <Button
                variant="secondary"
                className="flex-1"
                onClick={() => create(resolvedCwd)}
                disabled={createMutation.isPending}
              >
                <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} />
                {createMutation.isPending ? "Creating…" : "New session"}
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      variant="secondary"
                      size="icon"
                      aria-label="New session options"
                      title="New session options"
                    />
                  }
                >
                  <HugeiconsIcon icon={ChevronDownIcon} strokeWidth={2} />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-72">
                  <div className="flex flex-col gap-1.5 px-3 py-2.5">
                    <span className="px-1 text-xs font-medium text-muted-foreground">
                      Working directory
                    </span>
                    <CwdPicker
                      value={resolvedCwd}
                      // The picker feeds the local create button — a peer's
                      // paths aren't valid local dirs.
                      dirs={[
                        ...new Set(sessions.filter((s) => isLocalNode(s.node)).map((s) => s.cwd)),
                      ]}
                      onChange={setCwd}
                    />
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            </ButtonGroup>
          </>
        )}
      </div>

      <ScrollArea className="flex min-h-0 flex-1 flex-col" viewportRef={bodyScrollRef}>
        <SessionSections
          sessions={activeSessions}
          recentSessions={recentSessions}
          archivedSessions={archivedSessions}
          selectedId={selectedId}
          resolvedCwd={resolvedCwd}
          showContent={!loading && error === null}
          scrollRef={bodyScrollRef}
          hotkeyTarget={asideRef}
          onNewSession={setCreateCwd}
          onSelect={selectAndClose}
          onDetails={detailsAndClose}
          onDelete={onDeleteSession}
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
        {!loading && !error && filtered.length === 0 && sessions.length === 0 && <ListEmptyState />}
        {!loading && !error && filtered.length === 0 && sessions.length > 0 && (
          <EmptyScreen
            className="p-6"
            icon={SearchAreaIcon}
            title="No matches"
            description="No sessions match the current filters."
          />
        )}
      </ScrollArea>

      {newProjectFor !== null && (
        <ProjectNameDialog
          state={{ name: "" }}
          onClose={() => setNewProjectFor(null)}
          onSubmit={(_state, name) => {
            setNewProjectFor(null);
            createProject.mutate(
              // Create on the session's own node — a local-only project can't
              // hold a peer session (and the merged list keys it `node:id`).
              { name, node: newProjectSession?.node },
              {
                onSuccess: ({ project }) => {
                  if (newProjectSession === undefined) return;
                  const key = projectKey(project);
                  // Drop any prior reference to this project (bare or
                  // namespaced) so the merged list never sees it twice.
                  const ids = newProjectSession.projectIds.filter(
                    (id) => id !== key && bareProjectId(id) !== project.id,
                  );
                  patch.mutate({
                    // newProjectFor is the agent:id key — the API needs the bare id.
                    id: newProjectSession.id,
                    agent: newProjectSession.agent,
                    node: newProjectSession.node,
                    patch: { projectIds: [...ids, key] },
                  });
                },
              },
            );
          }}
        />
      )}
      <UserProfile />
    </Sidebar>
  );
}
