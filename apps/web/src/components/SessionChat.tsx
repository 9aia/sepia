import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { HistoryMessage } from "../lib/types";
import type { LiveMessage } from "../lib/liveMessages";
import { cancel, sendPrompt } from "../lib/api";
import { useHistory } from "../hooks/query/useHistory";
import { Button } from "./ui/button";
import { ErrorBanner } from "./ErrorBanner";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "./ui/message-scroller";
import { Message, MessageContent, MessageResponse } from "./ai-elements/message";
import { Reasoning, ReasoningContent, ReasoningTrigger } from "./ai-elements/reasoning";
import { Tool, ToolContent, ToolHeader } from "./ai-elements/tool";
import {
  PromptInput,
  PromptInputBody,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  type PromptInputMessage,
} from "./ai-elements/prompt-input";

type Row =
  | { readonly kind: "history"; readonly message: HistoryMessage }
  | { readonly kind: "live"; readonly message: LiveMessage };

function RowContent({ row }: { readonly row: Row }) {
  if (row.kind === "history") {
    const message = row.message;
    if (message.toolName !== undefined) {
      return (
        <Tool defaultOpen={false}>
          <ToolHeader
            type="dynamic-tool"
            toolName={message.toolName}
            state="output-available"
            title={message.toolName}
          />
          <ToolContent>
            <MessageResponse>{message.content}</MessageResponse>
          </ToolContent>
        </Tool>
      );
    }
    return (
      <Message from={message.role === "user" ? "user" : "assistant"}>
        <MessageContent>
          <MessageResponse>{message.content}</MessageResponse>
        </MessageContent>
      </Message>
    );
  }

  const message = row.message;
  if (message.role === "tool") {
    return (
      <Tool defaultOpen={false}>
        <ToolHeader
          type="dynamic-tool"
          toolName={message.toolName ?? "tool"}
          state={message.done ? "output-available" : "input-streaming"}
          title={message.toolName ?? "tool"}
        />
        <ToolContent>
          <MessageResponse>{message.content}</MessageResponse>
        </ToolContent>
      </Tool>
    );
  }
  if (message.role === "reasoning") {
    return (
      <Reasoning isStreaming={!message.done}>
        <ReasoningTrigger />
        <ReasoningContent>{message.content}</ReasoningContent>
      </Reasoning>
    );
  }
  return (
    <Message from={message.role === "user" ? "user" : "assistant"}>
      <MessageContent>
        <MessageResponse>{message.content}</MessageResponse>
      </MessageContent>
    </Message>
  );
}

/** Virtualized rows inside the message-scroller viewport. */
function ChatRows({
  rows,
  truncated,
}: {
  readonly rows: ReadonlyArray<Row>;
  readonly truncated: { readonly shown: number; readonly total: number };
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => 72,
    overscan: 10,
  });

  return (
    <MessageScrollerViewport ref={viewportRef}>
      <MessageScrollerContent>
        {truncated.shown < truncated.total && (
          <div className="text-center text-xs text-muted-foreground">
            showing last {truncated.shown} of {truncated.total}
          </div>
        )}
        <div className="relative mt-2.5" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const row = rows[virtualRow.index];
            if (row === undefined) return null;
            const isTurnStart = row.message.role === "user";
            const key =
              row.kind === "history"
                ? `h-${row.message.createdAt}-${virtualRow.index}`
                : `l-${row.message.id}`;
            return (
              <MessageScrollerItem
                key={key}
                messageId={key}
                scrollAnchor={isTurnStart}
                data-index={virtualRow.index}
                ref={virtualizer.measureElement}
                className="pb-2.5"
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${virtualRow.start}px)`,
                }}
              >
                <RowContent row={row} />
              </MessageScrollerItem>
            );
          })}
        </div>
      </MessageScrollerContent>
    </MessageScrollerViewport>
  );
}

interface SessionChatProps {
  readonly sessionId: string;
  readonly readOnly: boolean;
  readonly running: boolean;
  readonly liveMessages: ReadonlyArray<LiveMessage>;
  readonly onUserMessage: (text: string) => void;
  readonly onTakeover: () => void;
}

/**
 * The full conversation for a session: stored IR backlog + live streamed
 * turns in one scroll region, with a PromptInput composer at the bottom.
 */
export function SessionChat({
  sessionId,
  readOnly,
  running,
  liveMessages,
  onUserMessage,
  onTakeover,
}: SessionChatProps) {
  const historyQuery = useHistory(sessionId);
  const history = historyQuery.data?.messages ?? [];
  const [submitting, setSubmitting] = useState(false);
  const [promptError, setPromptError] = useState<string | null>(null);

  const rows = useMemo<Row[]>(
    () => [
      ...history.map((message): Row => ({ kind: "history", message })),
      ...liveMessages.map((message): Row => ({ kind: "live", message })),
    ],
    [history, liveMessages],
  );

  const onSubmit = (message: PromptInputMessage) => {
    const text = message.text.trim();
    if (text === "" || readOnly || running || submitting) return;
    setSubmitting(true);
    setPromptError(null);
    sendPrompt(sessionId, text)
      .then((ok) => {
        if (ok) onUserMessage(text);
        else setPromptError("Prompt failed.");
      })
      .catch((error: unknown) =>
        setPromptError(error instanceof Error ? error.message : "Prompt failed."),
      )
      .finally(() => setSubmitting(false));
  };

  return (
    <>
      <MessageScrollerProvider autoScroll defaultScrollPosition="end">
        <MessageScroller className="relative flex min-h-0 flex-1 flex-col">
          {rows.length > 0 ? (
            <ChatRows
              rows={rows}
              truncated={{ shown: history.length, total: historyQuery.data?.total ?? 0 }}
            />
          ) : (
            <MessageScrollerViewport>
              <p className="text-muted-foreground">
                {historyQuery.isLoading ? "Loading history…" : "No messages yet."}
              </p>
            </MessageScrollerViewport>
          )}
          <MessageScrollerButton direction="end" />
        </MessageScroller>
      </MessageScrollerProvider>

      {promptError !== null && <ErrorBanner>{promptError}</ErrorBanner>}

      {readOnly ? (
        <div className="flex items-center justify-between gap-3 border-t border-border px-4 py-3 text-sm text-muted-foreground">
          This session is held by another process.
          <Button size="sm" onClick={onTakeover}>
            Take over
          </Button>
        </div>
      ) : (
        <PromptInput onSubmit={onSubmit} className="shrink-0 border-t border-border pt-2.5">
          <PromptInputBody>
            <PromptInputTextarea placeholder="Prompt the agent…" />
          </PromptInputBody>
          <PromptInputFooter>
            <PromptInputTools />
            <PromptInputSubmit
              status={running ? "streaming" : submitting ? "submitted" : "ready"}
              disabled={submitting}
              onStop={() => void cancel(sessionId)}
            />
          </PromptInputFooter>
        </PromptInput>
      )}
    </>
  );
}
