import { lazy, Suspense, useEffect, useRef, useState, type CSSProperties } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useAppHotkey } from "../lib/keybinds";
import { useStore } from "@tanstack/react-store";
import { ChatPanel } from "../components/ChatPanel";
import { useSessions } from "../hooks/query/useSessions";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useSelfNode } from "../hooks/query/useNodes";
import { resolveSession, shouldSyncUrlSelection } from "../lib/format";
import { setDetailsFor } from "../lib/store";
import type { SessionSummary } from "../lib/types";
import { SessionList } from "../components/SessionList";
import { TokenGate } from "../components/TokenGate";
import { SidebarInset, SidebarProvider } from "../components/ui/sidebar";
import { useHealth } from "../hooks/query/useHealth";
import { useQueryClient } from "@tanstack/react-query";
import { AuthError } from "../lib/api";
import { sepiaStore, setSettingsOpen, setSelectedId } from "../lib/store";
import { sessionKey } from "../lib/format";

// The details drawer (checkpoints, resume targets, rename, transfer dialogs)
// is an on-demand surface — defer its chunk until a session is inspected.
const LazySessionDetailsDrawer = lazy(() =>
  import("../components/session-list/SessionDetailsDrawer").then((m) => ({
    default: m.SessionDetailsDrawer,
  })),
);

const DATES = new Set(["day", "week", "month"]);
const STATUSES = new Set(["free", "locked"]);
const SORTS = new Set(["newest", "oldest", "title"]);

export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>) => ({
    session: typeof search.session === "string" ? search.session : undefined,
    q: typeof search.q === "string" ? search.q : undefined,
    agents: typeof search.agents === "string" && search.agents !== "" ? search.agents : undefined,
    date: typeof search.date === "string" && DATES.has(search.date) ? search.date : undefined,
    status:
      typeof search.status === "string" && STATUSES.has(search.status) ? search.status : undefined,
    sort: typeof search.sort === "string" && SORTS.has(search.sort) ? search.sort : undefined,
  }),
  component: Home,
});

function Home() {
  const { data: sessions, error } = useSessions();
  const health = useHealth();
  const queryClient = useQueryClient();
  // Probes /api/node at boot — populates nodesStore.self/selfStatus and the
  // local node alias used by key resolution (previously only fetched while
  // Settings → Nodes was open).
  useSelfNode();
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  // URL → store: deep links and back/forward navigation. The sync flag
  // holds the store→URL effect back until the first commit — otherwise it
  // strips ?session= with the initial null selectedId before this effect's
  // setState lands.
  const [urlSynced, setUrlSynced] = useState(false);
  useEffect(() => {
    // Only a present ?session= pulls the store — never its absence. A
    // transition that briefly drops the param (stale commit, dropped
    // non-string value) used to null selectedId, and the auto-select
    // below then teleported the selection to the first row. Equivalent
    // key forms (agent:id ↔ node:agent:id ↔ local alias) don't count as
    // a change — string equality would ping-pong between them.
    if (shouldSyncUrlSelection(search.session, selectedId)) {
      setSelectedId(search.session ?? null);
    }
    setUrlSynced(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.session]);

  // Store → URL: keep ?session= in sync so the view is shareable.
  useEffect(() => {
    if (!urlSynced) return;
    if (selectedId === (search.session ?? null)) return;
    void navigate({
      to: "/",
      search: (prev) => ({ ...prev, session: selectedId ?? undefined }),
      replace: true,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, urlSynced]);

  // Canonicalize a resolvable selection to the row's own sessionKey once
  // the list lands — a deep link or a peer-minted key may carry a
  // different-but-equivalent form (agent:id, the server-issued local node
  // id); the canonical key keeps row highlighting and ?session= aligned.
  useEffect(() => {
    if (sessions === undefined || selectedId === null) return;
    const row = resolveSession(sessions, selectedId);
    if (row !== undefined) {
      const canonical = sessionKey(row);
      if (canonical !== selectedId) setSelectedId(canonical);
    }
  }, [sessions, selectedId]);

  // Auto-select the first session once the list lands, only when the user
  // hasn't picked one.
  useEffect(() => {
    const first = sessions?.[0];
    // search.session in the guard too — a deep link must not be overwritten
    // by the auto-select before the URL→store effect commits.
    if (selectedId === null && search.session === undefined && first !== undefined) {
      setSelectedId(sessionKey(first));
    }
  }, [sessions, selectedId, search.session]);

  useAppHotkey("app.keybinds", () => setSettingsOpen(true, "keyboard"));

  // Local-node recovery: the health ping runs every 10s. Once the API
  // answers again after an outage, refetch everything so the merged lists
  // and nodesStore.selfStatus heal without a restart — the UI degrades to
  // its "no nodes connected" state in the meantime, it never blocks.
  const wasUnreachable = useRef(false);
  useEffect(() => {
    if (health.isError) {
      wasUnreachable.current = true;
      return;
    }
    if (wasUnreachable.current && health.isSuccess) {
      wasUnreachable.current = false;
      void queryClient.invalidateQueries();
    }
  }, [health.isError, health.isSuccess, queryClient]);

  // Sticky gate: a focus/invalidation refetch clears `error` back to
  // pending while the 401 still stands — without the flag the gate
  // unmounts mid-cycle and the page flickers gate → skeleton → gate.
  const [authBlocked, setAuthBlocked] = useState(false);
  useEffect(() => {
    if (error instanceof AuthError) setAuthBlocked(true);
    else if (sessions !== undefined) setAuthBlocked(false);
  }, [error, sessions]);
  if (authBlocked || error instanceof AuthError) return <TokenGate />;

  return (
    <SidebarProvider style={{ "--sidebar-width": "24rem" } as CSSProperties}>
      <SessionList />
      <SidebarInset>
        <ChatPanel />
      </SidebarInset>
      <GlobalSessionDrawer />
    </SidebarProvider>
  );
}

/** The details drawer lives outside <Sidebar> — on mobile the sidebar is a
 * Sheet that unmounts its subtree when closed, which would take the drawer
 * (and its state) with it. */
function GlobalSessionDrawer() {
  const { data: sessions = [] } = useSessions();
  const details = useStore(sepiaStore, (state) => state.detailsFor);
  const deleteMutation = useDeleteSession();
  // Mount on first open, then keep mounted so the drawer's close animation
  // and internal state behave as if it had been mounted eagerly.
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    if (details !== null) setMounted(true);
  }, [details]);
  if (!mounted) return null;
  return (
    <Suspense fallback={null}>
      <LazySessionDetailsDrawer
        session={resolveSession(sessions, details?.id)}
        focusRename={details?.rename ?? false}
        onClose={() => setDetailsFor(null)}
        onOpen={setSelectedId}
        onDelete={(session: SessionSummary) =>
          deleteMutation.mutate({ id: session.id, agent: session.agent })
        }
      />
    </Suspense>
  );
}
