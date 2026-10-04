import { useEffect, type CSSProperties } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useHotkey } from "@tanstack/react-hotkeys";
import { useStore } from "@tanstack/react-store";
import { ChatPanel } from "../components/ChatPanel";
import { KeybindsDialog } from "../components/KeybindsDialog";
import { SessionList } from "../components/SessionList";
import { TokenGate } from "../components/TokenGate";
import { SidebarInset, SidebarProvider } from "../components/ui/sidebar";
import { useSessions } from "../hooks/query/useSessions";
import { AuthError } from "../lib/api";
import { sepiaStore, setKeybindsOpen, setSelectedId } from "../lib/store";
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
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  // URL → store: deep links and back/forward navigation.
  useEffect(() => {
    if ((search.session ?? null) !== selectedId) setSelectedId(search.session ?? null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.session]);

  // Store → URL: keep ?session= in sync so the view is shareable.
  useEffect(() => {
    if (selectedId === (search.session ?? null)) return;
    void navigate({
      to: "/",
      search: (prev) => ({ ...prev, session: selectedId ?? undefined }),
      replace: true,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId]);

  // Auto-select the first session once the list lands, only when the user
  // hasn't picked one.
  useEffect(() => {
    const first = sessions?.[0];
    if (selectedId === null && first !== undefined) setSelectedId(sessionKey(first));
  }, [sessions, selectedId]);

  useHotkey("Shift+[Slash]", () => setKeybindsOpen(true));

  if (error instanceof AuthError) return <TokenGate />;

  return (
    <SidebarProvider style={{ "--sidebar-width": "24rem" } as CSSProperties}>
      <SessionList />
      <SidebarInset>
        <ChatPanel />
      </SidebarInset>
      <KeybindsDialog />
    </SidebarProvider>
  );
}
