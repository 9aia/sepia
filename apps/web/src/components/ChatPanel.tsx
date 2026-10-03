import { useCallback, useEffect, useRef, useState } from "react";
import { CopilotKit } from "@copilotkit/react-core";
import { CopilotChat } from "@copilotkit/react-ui";
import { Markdown } from "@tanstack/markdown/react";
import { streamingMarkdownExtension } from "@tanstack/markdown/extensions/streaming";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useThrottledCallback } from "@tanstack/react-pacer";
import "@copilotkit/react-ui/styles.css";
import type { HistoryMessage, PermissionRequest, SessionSummary } from "../lib/types";
import { codeHighlighter, highlightThemeCss } from "../lib/highlight";
import { respondToPermission, subscribeSessionStream } from "../lib/api";
import type { StreamStatus } from "../lib/api";
import { ApprovalDialog } from "./ApprovalDialog";

interface ChatPanelProps {
  session: SessionSummary | null;
  history: HistoryMessage[];
  historyTotal: number;
  readOnly: boolean;
  attachError: string | null;
  historyError: string | null;
  onTakeover: (id: string) => void;
}

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

export function ChatPanel({
  session,
  history,
  historyTotal,
  readOnly,
  attachError,
  historyError,
  onTakeover,
}: ChatPanelProps) {
  const [mounted, setMounted] = useState(false);
  const [permission, setPermission] = useState<PermissionRequest | null>(null);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const sessionId = session?.id ?? null;
  const bodyRef = useRef<HTMLDivElement | null>(null);

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
    setPermission(null);
    if (!sessionId) return;
    return subscribeSessionStream(
      sessionId,
      (event) => {
        if (event.type !== "CUSTOM" || event.name !== PERMISSION_EVENT) return;
        const parsed = parsePermission(event.value);
        if (parsed) setPermission(parsed);
      },
      setStreamStatus,
    );
  }, [sessionId]);

  const resolvePermission = useCallback(
    (optionId: string | null) => {
      if (!sessionId || !permission) return;
      const requestId = permission.requestId;
      setPermission(null);
      void respondToPermission(sessionId, requestId, optionId);
    },
    [sessionId, permission],
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
        {readOnly && <span className="badge badge--readonly">read-only</span>}
        {session.busy && <span className="badge badge--busy">busy</span>}
        {streamStatus === "reconnecting" && (
          <span className="badge badge--reconnecting" role="status">
            reconnecting…
          </span>
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
                      <div className="history__content">
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
            <button
              type="button"
              className="chat-panel__takeover"
              onClick={() => onTakeover(session.id)}
            >
              Take over
            </button>
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
