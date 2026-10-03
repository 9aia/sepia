import { useCallback, useEffect, useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { ChatPanel } from "../components/ChatPanel";
import { SessionList } from "../components/SessionList";
import { attach, createSession, deleteSession, getHistory, isMock, listSessions } from "../lib/api";
import type { CreateSessionInput, HistoryMessage, SessionSummary } from "../lib/types";

export const Route = createFileRoute("/")({
  component: Home,
});

function Home() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [history, setHistory] = useState<HistoryMessage[]>([]);
  const [historyTotal, setHistoryTotal] = useState(0);
  const [readOnly, setReadOnly] = useState(false);
  const [attachError, setAttachError] = useState<string | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const selectSession = useCallback((id: string) => {
    setSelectedId(id);
    setHistory([]);
    setHistoryTotal(0);
    setReadOnly(false);
    setAttachError(null);
    setHistoryError(null);
    attach(id)
      .then((result) => setReadOnly(result.readOnly))
      .catch((err: unknown) => {
        setAttachError(err instanceof Error ? err.message : "Failed to attach session");
      });
    getHistory(id)
      .then((page) => {
        setHistory(page.messages);
        setHistoryTotal(page.total);
      })
      .catch((err: unknown) => {
        setHistory([]);
        setHistoryTotal(0);
        setHistoryError(err instanceof Error ? err.message : "Failed to load history");
      });
  }, []);

  const remove = useCallback(
    (id: string) => {
      deleteSession(id)
        .then(() => {
          setSessions((items) => items.filter((session) => session.id !== id));
          if (selectedId === id) {
            setSelectedId(null);
            setHistory([]);
            setHistoryTotal(0);
            setReadOnly(false);
            setAttachError(null);
            setHistoryError(null);
          }
        })
        .catch((err: unknown) => {
          setError(err instanceof Error ? err.message : "Failed to delete session");
        });
    },
    [selectedId],
  );

  const takeover = useCallback(
    (id: string) => {
      setAttachError(null);
      attach(id, { takeover: true })
        .then((result) => setReadOnly(result.readOnly))
        .catch((err: unknown) => {
          setAttachError(err instanceof Error ? err.message : "Takeover failed");
        });
    },
    [setReadOnly],
  );

  useEffect(() => {
    let active = true;
    setLoading(true);
    listSessions()
      .then((items) => {
        if (!active) return;
        setSessions(items);
        setError(null);
      })
      .catch((err: unknown) => {
        if (!active) return;
        setError(err instanceof Error ? err.message : "Failed to load sessions");
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const create = useCallback(
    (input: CreateSessionInput) => {
      setCreating(true);
      setCreateError(null);
      createSession(input)
        .then(({ id }) =>
          listSessions().then((items) => {
            setSessions(items);
            selectSession(id);
          }),
        )
        .catch((err: unknown) => {
          setCreateError(err instanceof Error ? err.message : "Failed to create session");
        })
        .finally(() => setCreating(false));
    },
    [selectSession],
  );

  const selected = sessions.find((s) => s.id === selectedId) ?? null;

  return (
    <div className="app-shell">
      {isMock && (
        <div className="mock-banner" role="status">
          MOCK DATA — not connected to the API
        </div>
      )}
      <SessionList
        sessions={sessions}
        selectedId={selectedId}
        loading={loading}
        error={error}
        creating={creating}
        createError={createError}
        onSelect={selectSession}
        onCreate={create}
        onDelete={remove}
      />
      <ChatPanel
        session={selected}
        history={history}
        historyTotal={historyTotal}
        readOnly={readOnly}
        attachError={attachError}
        historyError={historyError}
        onTakeover={takeover}
      />
    </div>
  );
}
