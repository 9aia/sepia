import { useEffect, useState, type CSSProperties } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useAppHotkey } from "../lib/keybinds";
import { useStore } from "@tanstack/react-store";
import { ChatPanel } from "../components/ChatPanel";
import { SessionDetailsDrawer } from "../components/session-list/SessionDetailsDrawer";
import { useSessions } from "../hooks/query/useSessions";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { resolveSession, shouldSyncUrlSelection } from "../lib/format";
import { setDetailsFor } from "../lib/store";
import type { SessionSummary } from "../lib/types";
import { SessionList } from "../components/SessionList";
import { TokenGate } from "../components/TokenGate";
import { SidebarInset, SidebarProvider } from "../components/ui/sidebar";
import { useHealth } from "../hooks/query/useHealth";
import { useQueryClient } from "@tanstack/react-query";
import { EmptyScreen } from "../components/EmptyScreen";
import { Button } from "../components/ui/button";
import { CloudOffIcon } from "@hugeicons/core-free-icons";
import { AuthError } from "../lib/api";
import { sepiaStore, setSettingsOpen, setSelectedId } from "../lib/store";
import { sessionKey } from "../lib/format";

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

  if (error instanceof AuthError) return <TokenGate />;

  // The PWA shell still loads when the API is down — say so instead of an
  // empty app. Only when sessions also failed: a transient health blip
  // shouldn't wipe the UI (the footer dot stays the transient indicator).
  if (health.isError && sessions === undefined && error !== undefined) {
    return (
      <main className="flex min-h-svh">
        <EmptyScreen
          className="m-auto max-w-xl"
          icon={CloudOffIcon}
          title="Server unreachable"
          description="The Sepia server isn't responding — make sure `sepia serve` is running, then retry."
        >
          <Button
            variant="secondary"
            onClick={() => void queryClient.invalidateQueries()}
            disabled={health.isFetching}
          >
            {health.isFetching ? "Retrying…" : "Retry"}
          </Button>
        </EmptyScreen>
      </main>
    );
  }

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
  return (
    <SessionDetailsDrawer
      session={resolveSession(sessions, details?.id)}
      focusRename={details?.rename ?? false}
      onClose={() => setDetailsFor(null)}
      onOpen={setSelectedId}
      onDelete={(session: SessionSummary) =>
        deleteMutation.mutate({ id: session.id, agent: session.agent })
      }
    />
  );
}
