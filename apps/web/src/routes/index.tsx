import { useEffect } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useStore } from "@tanstack/react-store";
import { ChatPanel } from "../components/ChatPanel";
import { SessionList } from "../components/SessionList";
import { useSessions } from "../hooks/query/useSessions";
import { sepiaStore, setSelectedId } from "../lib/store";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const { data: sessions } = useSessions();
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);

  // Auto-select the first session once the list lands, only when the user
  // hasn't picked one.
  useEffect(() => {
    const first = sessions?.[0];
    if (selectedId === null && first !== undefined) setSelectedId(first.id);
  }, [sessions, selectedId]);

  return (
    <div className="app-shell">
      <SessionList />
      <ChatPanel />
    </div>
  );
}
