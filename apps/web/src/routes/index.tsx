import { useEffect } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { ChatPanel } from "../components/ChatPanel";
import { SessionList } from "../components/SessionList";
import { isMock } from "../lib/api";
import { loadAgents, refreshSessions } from "../lib/store";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  useEffect(() => {
    refreshSessions();
    loadAgents();
  }, []);

  return (
    <div className="layout">
      {isMock && <div className="mock-banner">MOCK DATA — no API server connected</div>}
      <SessionList />
      <ChatPanel />
    </div>
  );
}
