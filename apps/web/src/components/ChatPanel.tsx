import { useCallback, useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import type { PermissionRequest } from "../lib/types";
import { subscribeSessionStream } from "../lib/api";
import type { StreamStatus } from "../lib/api";
import { applyAguiEvent, type LiveMessage } from "../lib/liveMessages";
import { sepiaStore } from "../lib/store";
import { queryKeys } from "../hooks/query/keys";
import { useAttachSession } from "../hooks/query/useAttachSession";
import { useHistory } from "../hooks/query/useHistory";
import { useRespondToPermission } from "../hooks/query/useRespondToPermission";
import { useSessions } from "../hooks/query/useSessions";
import { ApprovalDialog } from "./ApprovalDialog";
import { Badge } from "./ui/badge";
import { SessionChat } from "./SessionChat";

const PERMISSION_EVENT = "acp:permission_request";

function parsePermission(value: unknown): PermissionRequest | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.requestId !== "string") return null;
  const options = Array.isArray(raw.options) ? raw.options : [];
  return {
    requestId: raw.requestId,
    title: typeof raw.title === "string" ? raw.title : "Permission requested",
    options: options
      .filter((o): o is Record<string, unknown> => !!o && typeof o === "object")
      .map((o) => {
        const optionId = typeof o.optionId === "string" ? o.optionId : "";
        const label = typeof o.label === "string" ? o.label : optionId;
        return {
          optionId,
          label,
          kind: typeof o.kind === "string" ? o.kind : undefined,
        };
      })
      .filter((o) => o.optionId !== ""),
  };
}

const messageOf = (err: unknown, fallback: string): string =>
  err instanceof Error ? err.message : fallback;

export function ChatPanel() {
  const selectedId = useStore(sepiaStore, (state) => state.selectedId);
  const { data: sessions = [] } = useSessions();
  const session = sessions.find((s) => s.id === selectedId) ?? null;
  const sessionId = session?.id ?? null;

  const queryClient = useQueryClient();
  const attachMutation = useAttachSession();
  const respondMutation = useRespondToPermission();
  const historyQuery = useHistory(sessionId);

  // Mutation state is for the last mutate() call; only trust it when it
  // refers to the session currently on screen.
  const forCurrent = attachMutation.variables?.id === sessionId;
  const attachReady = attachMutation.isSuccess && forCurrent && attachMutation.data.attached;
  const readOnly = forCurrent && attachMutation.isSuccess && attachMutation.data.readOnly;
  const attachError =
    attachMutation.isError && forCurrent
      ? messageOf(attachMutation.error, "Failed to attach session")
      : null;
  const historyError = historyQuery.isError
    ? messageOf(historyQuery.error, "Failed to load history")
    : null;

  const [permission, setPermission] = useState<PermissionRequest | null>(null);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [running, setRunning] = useState(false);
  const [liveMessages, setLiveMessages] = useState<LiveMessage[]>([]);
  const { mutate: attach } = attachMutation;

  useEffect(() => {
    if (sessionId !== null) attach({ id: sessionId });
  }, [sessionId, attach]);

  // Live rows are optimistic: once the run ends and the refetched IR backlog
  // covers them, they would duplicate — clear whenever fresh history lands
  // while no run is active.
  useEffect(() => {
    if (!running && historyQuery.data !== undefined) setLiveMessages([]);
  }, [running, historyQuery.data]);

  useEffect(() => {
    setPermission(null);
    setRunning(false);
    setLiveMessages([]);
    // The stream subscribes to live-session events; it only exists once the
    // agent is attached, otherwise every request just races a 400.
    if (!sessionId || !attachReady) return;
    return subscribeSessionStream(
      sessionId,
      (event) => {
        if (event.type === "CUSTOM" && event.name === PERMISSION_EVENT) {
          const parsed = parsePermission(event.value);
          if (parsed) setPermission(parsed);
          return;
        }
        setLiveMessages((messages) => applyAguiEvent(messages, event));
        // The SSE feed mirrors the live turn lifecycle; no polling needed.
        if (event.type === "RUN_STARTED") setRunning(true);
        if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
          setRunning(false);
          void queryClient.invalidateQueries({ queryKey: queryKeys.history(sessionId) });
        }
      },
      setStreamStatus,
    );
  }, [sessionId, attachReady, queryClient]);

  const resolvePermission = useCallback(
    (optionId: string | null) => {
      if (!sessionId || !permission) return;
      const requestId = permission.requestId;
      setPermission(null);
      respondMutation.mutate({ sessionId, requestId, optionId });
    },
    [sessionId, permission, respondMutation],
  );

  const addUserMessage = useCallback((text: string) => {
    setLiveMessages((messages) => [
      ...messages,
      { id: `user-${Date.now()}`, role: "user", content: text, done: true },
    ]);
  }, []);

  if (!session) {
    return (
      <section className="chat-panel chat-panel--empty">
        <p>Select a session to view its conversation.</p>
      </section>
    );
  }

  return (
    <section className="chat-panel">
      <header className="chat-panel__header">
        <div>
          <h2 className="chat-panel__title">{session.title}</h2>
          <span className="chat-panel__cwd" title={session.cwd}>
            {session.cwd}
          </span>
        </div>
        {readOnly && <Badge variant="secondary">read-only</Badge>}
        {(session.busy || running) && <Badge variant="destructive">busy</Badge>}
        {streamStatus === "reconnecting" && (
          <Badge variant="outline" role="status">
            reconnecting…
          </Badge>
        )}
      </header>

      <div className="chat-panel__body">
        {attachError !== null && (
          <div className="chat-panel__error" role="alert">
            {attachError}
          </div>
        )}
        {historyError !== null && (
          <div className="chat-panel__error" role="alert">
            {historyError}
          </div>
        )}
        {attachError === null && (
          <SessionChat
            sessionId={session.id}
            readOnly={readOnly}
            running={running || session.busy}
            liveMessages={liveMessages}
            onUserMessage={addUserMessage}
            onTakeover={() => attach({ id: session.id, takeover: true })}
          />
        )}
      </div>

      <ApprovalDialog request={permission} onResolve={resolvePermission} />
    </section>
  );
}
