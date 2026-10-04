import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { BubbleChatIcon } from "@hugeicons/core-free-icons";
import { EmptyScreen } from "./EmptyScreen";
import type { PermissionRequest } from "../lib/types";
import { subscribeSessionStream } from "../lib/api";
import type { StreamStatus } from "../lib/api";
import { applyAguiEvent, type LiveMessage } from "../lib/liveMessages";
import { liveCoveredByHistory } from "../lib/historyRows";
import { sepiaStore } from "../lib/store";
import { settingsStore } from "../lib/settings";
import { modelArgsFor } from "../lib/models";
import { resolveSession } from "../lib/format";
import { queryKeys } from "../hooks/query/keys";
import { useAttachSession } from "../hooks/query/useAttachSession";
import { flattenHistory, useHistory } from "../hooks/query/useHistory";
import { useRespondToPermission } from "../hooks/query/useRespondToPermission";
import { useSessions } from "../hooks/query/useSessions";
import { ApprovalDialog } from "./ApprovalDialog";
import { ChatHeader } from "./ChatHeader";
import { ErrorBanner } from "./ErrorBanner";
import { SessionChat } from "./SessionChat";
import { ChatSkeleton } from "./ChatSkeleton";

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
  const { data: sessions = [], isLoading: sessionsLoading } = useSessions();
  const session = resolveSession(sessions, selectedId) ?? null;
  const sessionId = session?.id ?? null;

  const queryClient = useQueryClient();
  const attachMutation = useAttachSession();
  const respondMutation = useRespondToPermission();
  const historyQuery = useHistory(sessionId, session?.agent);

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

  const [permissions, setPermissions] = useState<PermissionRequest[]>([]);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [running, setRunning] = useState(false);
  const [liveMessages, setLiveMessages] = useState<LiveMessage[]>([]);
  const { mutate: attach } = attachMutation;
  const settings = useStore(settingsStore);
  // Only the model args should re-trigger attach — the settings object
  // identity changes on any pref write (theme, keybinds) and would
  // re-attach the session on every change.
  const modelArgs = useMemo(
    () => modelArgsFor(session?.agent ?? "", session?.model, settings),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [session?.agent, session?.model, settings.models],
  );
  useEffect(() => {
    if (sessionId === null) return;
    attach({ id: sessionId, agent: session?.agent, ...modelArgs });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, attach, modelArgs]);

  // Live rows are optimistic: once the run ends, the refetched IR backlog
  // duplicates them. Clear them only once every live user/assistant text is
  // actually present in fresh history — clearing earlier causes flicker.
  const historyMessages = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  useEffect(() => {
    if (running || historyMessages.length === 0) return;
    setLiveMessages((live) => (liveCoveredByHistory(live, historyMessages) ? [] : live));
  }, [running, historyMessages]);

  useEffect(() => {
    setPermissions([]);
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
          if (parsed) {
            setPermissions((prev) =>
              prev.some((p) => p.requestId === parsed.requestId) ? prev : [...prev, parsed],
            );
          }
          return;
        }
        setLiveMessages((messages) => applyAguiEvent(messages, event));
        // The SSE feed mirrors the live turn lifecycle; no polling needed.
        if (event.type === "RUN_STARTED") setRunning(true);
        if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
          setRunning(false);
          const refetch = (): void => {
            void queryClient.invalidateQueries({
              queryKey: queryKeys.history(sessionId, session?.agent),
            });
          };
          refetch();
          // The agent flushes the IR asynchronously — a second pass picks up
          // messages that weren't in the store on the first refetch, or the
          // live rows would ghost until a manual refresh.
          setTimeout(refetch, 2000);
        }
      },
      setStreamStatus,
      session?.agent,
    );
  }, [sessionId, attachReady, queryClient, session?.agent]);

  const resolvePermissions = useCallback(
    (answers: Readonly<Record<string, string>>) => {
      if (!sessionId || permissions.length === 0) return;
      const pending = permissions;
      setPermissions([]);
      for (const request of pending) {
        respondMutation.mutate({
          sessionId,
          agent: session?.agent,
          requestId: request.requestId,
          optionId: answers[request.requestId] ?? null,
        });
      }
    },
    [sessionId, session?.agent, permissions, respondMutation],
  );

  const cancelPermissions = useCallback(() => {
    resolvePermissions({});
  }, [resolvePermissions]);

  // Optimistic — returns the id so a failed send can roll the row back.
  const addUserMessage = useCallback((text: string): string => {
    const id = `user-${Date.now()}`;
    setLiveMessages((messages) => [...messages, { id, role: "user", content: text, done: true }]);
    return id;
  }, []);
  const removeLiveMessage = useCallback((id: string): void => {
    setLiveMessages((messages) => messages.filter((m) => m.id !== id));
  }, []);

  if (!session) {
    return (
      <section className="flex h-svh flex-col overflow-hidden">
        {sessionsLoading ? (
          <ChatSkeleton />
        ) : (
          <EmptyScreen
            icon={BubbleChatIcon}
            title="No session selected"
            description="Pick a session from the sidebar, or create a new one."
          />
        )}
      </section>
    );
  }

  return (
    <section className="flex h-svh flex-col overflow-hidden">
      <ChatHeader session={session} running={running} />

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {attachError !== null && <ErrorBanner>{attachError}</ErrorBanner>}
        {historyError !== null && <ErrorBanner>{historyError}</ErrorBanner>}
        {attachError === null && (
          <SessionChat
            sessionId={session.id}
            agent={session.agent}
            readOnly={readOnly}
            running={running || session.busy}
            liveMessages={liveMessages}
            streamStatus={streamStatus}
            onUserMessage={addUserMessage}
            onRemoveLiveMessage={removeLiveMessage}
            onTakeover={() =>
              attach({
                id: session.id,
                agent: session.agent,
                takeover: true,
                ...modelArgsFor(session.agent, session.model, settings),
              })
            }
          />
        )}
      </div>

      <ApprovalDialog
        requests={permissions}
        onResolve={resolvePermissions}
        onCancel={cancelPermissions}
      />
    </section>
  );
}
