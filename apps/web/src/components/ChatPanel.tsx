import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import { AlertCircleIcon, BubbleChatIcon } from "@hugeicons/core-free-icons";
import { EmptyScreen } from "./EmptyScreen";
import type { HistoryBlock, PermissionRequest } from "../lib/types";
import { listSessions, subscribeSessionStream } from "../lib/api";
import type { StreamStatus } from "../lib/api";
import { historyKeyMatches } from "../lib/events";
import { nodeTarget } from "../lib/nodes";
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
import { Button } from "./ui/button";
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
  const historyQuery = useHistory(sessionId, session?.agent, session?.node);

  // Mutation state is for the last mutate() call; only trust it when it
  // refers to the session currently on screen.
  const forCurrent = attachMutation.variables?.id === sessionId;
  const attachReady = attachMutation.isSuccess && forCurrent && attachMutation.data.attached;
  const takeoverAttempted = forCurrent && attachMutation.variables?.takeover === true;
  // A failed takeover leaves the session held — the mutation's error state
  // must not silently flip the composer back to writable, or the next send
  // just 400s on a still-detached session.
  const readOnly =
    forCurrent && attachMutation.isSuccess ? attachMutation.data.readOnly : takeoverAttempted;
  // The agent's advertised prompt capabilities — probed at attach. Absent
  // for older peers and failed attaches; the composer only hides what an
  // explicit `false` rules out.
  const promptCapabilities =
    forCurrent && attachMutation.isSuccess
      ? attachMutation.data.capabilities?.promptCapabilities
      : undefined;
  // A takeover failure stays in the chat (the session is still held) and is
  // reported inside the takeover dialog — it must not collapse the panel
  // into the full-screen attach error.
  const attachError =
    attachMutation.isError && forCurrent && !takeoverAttempted
      ? messageOf(attachMutation.error, "Failed to attach session")
      : null;
  const takeoverError = takeoverAttempted
    ? attachMutation.isError
      ? messageOf(attachMutation.error, "Takeover failed")
      : attachMutation.isSuccess && !attachMutation.data.attached
        ? "The session is still held — the other process didn't let go."
        : null
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
    attach({ id: sessionId, agent: session?.agent, node: session?.node, ...modelArgs });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, attach, modelArgs]);

  // A held session has no live stream — the other process's updates never
  // reach this node — so the panel polls the node's lock view instead. Each
  // tick re-syncs the transcript (rows the holder flushes keep appearing)
  // and refreshes the holder pid shown in the takeover dialog; when the
  // lock clears the session attaches on its own, which flips `readOnly`
  // off and opens the real stream. `refetchInterval` pauses in background
  // tabs, so the probe only runs while the session is actually on screen.
  const heldPoll = useQuery({
    queryKey: ["held-session", session?.node ?? "", session?.agent ?? "", sessionId ?? ""],
    enabled: readOnly && sessionId !== null,
    staleTime: 0,
    gcTime: 0,
    refetchInterval: 6_000,
    queryFn: async () => {
      const rows = await listSessions(nodeTarget(session?.node), { withLocks: true });
      return rows.find((row) => row.id === sessionId) ?? null;
    },
  });

  // Attach when the lock is seen released. `heldWasLocked` guards the
  // locked→free edge so a poll whose probe can't see this backend's locks
  // (e.g. a devin probe listing a cline store) doesn't fire an attach on
  // every tick — it still gets one shot, which also covers a holder that
  // let go between the failed attach and the first poll.
  const heldWasLocked = useRef<boolean | null>(null);
  useEffect(() => {
    const row = heldPoll.data;
    if (!readOnly || sessionId === null || row === undefined || row === null) return;
    if (row.locked) {
      heldWasLocked.current = true;
      return;
    }
    const released = heldWasLocked.current !== false;
    heldWasLocked.current = false;
    if (!released || attachMutation.isPending) return;
    attach({ id: sessionId, agent: session?.agent, node: session?.node, ...modelArgs });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [heldPoll.data, readOnly, sessionId, attachMutation.isPending]);

  // Every successful poll invalidates this session's history — the held
  // session reads as a live transcript while the holder writes to the
  // store.
  useEffect(() => {
    if (!readOnly || sessionId === null || heldPoll.dataUpdatedAt === 0) return;
    void queryClient.invalidateQueries({
      queryKey: ["history"],
      predicate: (query) => historyKeyMatches(query.queryKey[1], sessionId),
    });
  }, [heldPoll.dataUpdatedAt, readOnly, sessionId, queryClient]);

  // Live rows are optimistic: once the run ends, the refetched IR backlog
  // duplicates them. Clear them only once every live user/assistant text is
  // actually present in fresh history — clearing earlier causes flicker.
  const historyMessages = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  useEffect(() => {
    if (running || historyMessages.length === 0) return;
    setLiveMessages((live) => (liveCoveredByHistory(live, historyMessages) ? [] : live));
  }, [running, historyMessages]);

  // Clear per-session state only when the session actually changes. The
  // effect also re-runs when attachReady flips (e.g. after a takeover) —
  // clearing there would wipe the optimistic row a held send just added
  // (child effects run before this parent's).
  const clearedFor = useRef<string | null>(null);
  useEffect(() => {
    if (clearedFor.current !== sessionId) {
      clearedFor.current = sessionId;
      heldWasLocked.current = null;
      setPermissions([]);
      setRunning(false);
      setLiveMessages([]);
    }
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
              queryKey: queryKeys.history(sessionId, session?.agent, session?.node),
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
      // Routed like every other session op — a peer's stream reaches it
      // through its target (direct, or /api/gateway for via: "gateway").
      nodeTarget(session?.node),
    );
  }, [sessionId, attachReady, queryClient, session?.agent, session?.node]);

  const resolvePermissions = useCallback(
    (answers: Readonly<Record<string, string>>) => {
      if (!sessionId || permissions.length === 0) return;
      const pending = permissions;
      setPermissions([]);
      for (const request of pending) {
        respondMutation.mutate({
          sessionId,
          agent: session?.agent,
          node: session?.node,
          requestId: request.requestId,
          optionId: answers[request.requestId] ?? null,
        });
      }
    },
    [sessionId, session?.agent, session?.node, permissions, respondMutation],
  );

  const cancelPermissions = useCallback(() => {
    resolvePermissions({});
  }, [resolvePermissions]);

  // Optimistic — returns the id so a failed send can roll the row back.
  const addUserMessage = useCallback(
    (text: string, blocks?: ReadonlyArray<HistoryBlock>): string => {
      const id = `user-${Date.now()}`;
      setLiveMessages((messages) => [
        ...messages,
        {
          id,
          createdAt: Date.now(),
          role: "user",
          content: text,
          ...(blocks !== undefined ? { blocks } : {}),
          done: true,
        },
      ]);
      return id;
    },
    [],
  );
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
        {attachError !== null ? (
          <EmptyScreen
            icon={AlertCircleIcon}
            title="Couldn't load the session"
            description={attachError}
          >
            <Button
              variant="secondary"
              onClick={() => {
                attachMutation.reset();
                attach({
                  id: sessionId!,
                  agent: session?.agent,
                  node: session?.node,
                  ...modelArgs,
                });
                void queryClient.invalidateQueries({
                  queryKey: queryKeys.history(sessionId!, session?.agent, session?.node),
                });
              }}
            >
              Try again
            </Button>
          </EmptyScreen>
        ) : (
          <>
            {historyError !== null && <ErrorBanner>{historyError}</ErrorBanner>}
            <SessionChat
              sessionId={session.id}
              agent={session.agent}
              readOnly={readOnly}
              promptCapabilities={promptCapabilities}
              running={running || session.busy}
              liveMessages={liveMessages}
              streamStatus={streamStatus}
              onUserMessage={addUserMessage}
              onRemoveLiveMessage={removeLiveMessage}
              takeoverError={takeoverError}
              takeoverPending={takeoverAttempted && attachMutation.isPending}
              holderPid={heldPoll.data?.lockHolderPid ?? null}
              onTakeover={() =>
                attach({
                  id: session.id,
                  agent: session.agent,
                  node: session.node,
                  takeover: true,
                  ...modelArgsFor(session.agent, session.model, settings),
                })
              }
              onReattach={() =>
                attachMutation
                  .mutateAsync({
                    id: session.id,
                    agent: session.agent,
                    node: session.node,
                    ...modelArgsFor(session.agent, session.model, settings),
                  })
                  .then((result) => result.attached)
              }
            />
          </>
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
