import { useCallback, useEffect, useRef, useState } from "react";
import { CopilotKit } from "@copilotkit/react-core";
import { CopilotChat } from "@copilotkit/react-ui";
import { Markdown } from "@tanstack/markdown/react";
import { streamingMarkdownExtension } from "@tanstack/markdown/extensions/streaming";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useThrottledCallback } from "@tanstack/react-pacer";
import "@copilotkit/react-ui/styles.css";
import { useQueryClient } from "@tanstack/react-query";
import { useStore } from "@tanstack/react-store";
import type { PermissionRequest } from "../lib/types";
import { codeHighlighter, highlightThemeCss } from "../lib/highlight";
import { subscribeSessionStream } from "../lib/api";
import type { StreamStatus } from "../lib/api";
import { sepiaStore } from "../lib/store";
import { queryKeys } from "../hooks/query/keys";
import { useAttachSession } from "../hooks/query/useAttachSession";
import { useHistory } from "../hooks/query/useHistory";
import { useRespondToPermission } from "../hooks/query/useRespondToPermission";
import { useSessions } from "../hooks/query/useSessions";
import { ApprovalDialog } from "./ApprovalDialog";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";

const PERMISSION_EVENT = "acp:permission_request";

// The streaming profile also suits stored history: it suppresses dangling
// trailing blocks and keeps raw HTML/javascript: URLs out of agent output.
const markdownExtensions = [streamingMarkdownExtension()];

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
  const history = historyQuery.data?.messages ?? [];
  const historyTotal = historyQuery.data?.total ?? 0;

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

  const [mounted, setMounted] = useState(false);
  const [permission, setPermission] = useState<PermissionRequest | null>(null);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [running, setRunning] = useState(false);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const { mutate: attach } = attachMutation;

  const historyVirtualizer = useVirtualizer({
    count: history.length,
    getScrollElement: () => bodyRef.current,
    estimateSize: () => 72,
    overscan: 10,
  });

  const scrollToBottom = useThrottledCallback(
    () => {
      const el = bodyRef.current;
      if (el !== null) el.scrollTop = el.scrollHeight;
    },
    { wait: 150, leading: true, trailing: true },
  );

  useEffect(() => {
    setMounted(true);
  }, []);

  useEffect(() => {
    if (history.length > 0) scrollToBottom();
  }, [history.length, sessionId, scrollToBottom]);

  useEffect(() => {
    if (sessionId !== null) attach({ id: sessionId });
  }, [sessionId, attach]);

  useEffect(() => {
    setPermission(null);
    setRunning(false);
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

      <div className="chat-panel__body" ref={bodyRef}>
        <style>{highlightThemeCss}</style>
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
        {history.length > 0 && (
          <div className="history">
            {history.length < historyTotal && (
              <div className="history__truncated">
                showing last {history.length} of {historyTotal}
              </div>
            )}
            <div
              className="history__rows"
              style={{ height: `${historyVirtualizer.getTotalSize()}px`, position: "relative" }}
            >
              {historyVirtualizer.getVirtualItems().map((row) => {
                const message = history[row.index];
                if (message === undefined) return null;
                return (
                  <div
                    key={`${message.createdAt}-${row.index}`}
                    data-index={row.index}
                    ref={historyVirtualizer.measureElement}
                    className="history__row"
                    style={{
                      position: "absolute",
                      top: 0,
                      left: 0,
                      width: "100%",
                      transform: `translateY(${row.start}px)`,
                    }}
                  >
                    <div className={`history__message history__message--${message.role}`}>
                      <div className="history__meta">
                        <span className="history__role">
                          {message.toolName ? `${message.role}:${message.toolName}` : message.role}
                        </span>
                      </div>
                      <div className="history__content prose prose-invert prose-sm">
                        <Markdown
                          extensions={markdownExtensions}
                          frontmatter={false}
                          headingIds={false}
                          highlighter={codeHighlighter}
                        >
                          {message.content}
                        </Markdown>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {attachError !== null ? null : readOnly ? (
          <div className="chat-panel__readonly">
            This session is held by another process.
            <Button size="sm" onClick={() => attach({ id: session.id, takeover: true })}>
              Take over
            </Button>
          </div>
        ) : mounted ? (
          <CopilotKit
            key={session.id}
            runtimeUrl={`/api/copilotkit?sessionId=${encodeURIComponent(session.id)}`}
          >
            <CopilotChat
              className="copilot-chat"
              labels={{ title: session.title, initial: "Ask about this session." }}
            />
          </CopilotKit>
        ) : (
          <div className="chat-panel__loading">Loading chat…</div>
        )}
      </div>

      <ApprovalDialog request={permission} onResolve={resolvePermission} />
    </section>
  );
}
