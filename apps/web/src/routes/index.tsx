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

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const { data: sessions, error } = useSessions();
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);

  // Auto-select the first session once the list lands, only when the user
  // hasn't picked one.
  useEffect(() => {
    const first = sessions?.[0];
    if (selectedId === null && first !== undefined) setSelectedId(first.id);
  }, [sessions, selectedId]);

  useHotkey("Shift+[Slash]", () => setKeybindsOpen(true));

  if (error instanceof AuthError) return <TokenGate />;

  return (
    <SidebarProvider style={{ "--sidebar-width": "20rem" } as CSSProperties}>
      <SessionList />
      <SidebarInset>
        <ChatPanel />
      </SidebarInset>
      <KeybindsDialog />
    </SidebarProvider>
  );
}
