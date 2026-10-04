import { useMemo, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useStickToBottomContext } from "use-stick-to-bottom";
import type { HistoryMessage } from "../lib/types";
import type { LiveMessage } from "../lib/liveMessages";
import { cancel, sendPrompt } from "../lib/api";
import { useHistory } from "../hooks/query/useHistory";
import { Button } from "./ui/button";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "./ai-elements/conversation";
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

/** Virtualized rows; lives inside Conversation so it can borrow its scroller. */
function ChatRows({
  rows,
  truncated,
}: {
  readonly rows: ReadonlyArray<Row>;
  readonly truncated: { readonly shown: number; readonly total: number };
}) {
  const { scrollRef } = useStickToBottomContext();
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current as HTMLElement | null,
    estimateSize: () => 72,
    overscan: 10,
  });

  return (
    <ConversationContent>
      {truncated.shown < truncated.total && (
        <div className="history__truncated">
          showing last {truncated.shown} of {truncated.total}
        </div>
      )}
      <div
        className="history__rows"
        style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}
      >
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const row = rows[virtualRow.index];
          if (row === undefined) return null;
          const key =
            row.kind === "history"
              ? `h-${row.message.createdAt}-${virtualRow.index}`
              : `l-${row.message.id}`;
          return (
            <div
              key={key}
              data-index={virtualRow.index}
              ref={virtualizer.measureElement}
              className="history__row"
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${virtualRow.start}px)`,
              }}
            >
              <RowContent row={row} />
            </div>
          );
        })}
      </div>
    </ConversationContent>
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
      <Conversation className="chat-conversation">
        {rows.length > 0 ? (
          <ChatRows
            rows={rows}
            truncated={{ shown: history.length, total: historyQuery.data?.total ?? 0 }}
          />
        ) : (
          <ConversationContent>
            <p className="chat-panel__loading">
              {historyQuery.isLoading ? "Loading history…" : "No messages yet."}
            </p>
          </ConversationContent>
        )}
        <ConversationScrollButton />
      </Conversation>

      {promptError !== null && (
        <div className="chat-panel__error" role="alert">
          {promptError}
        </div>
      )}

      {readOnly ? (
        <div className="chat-panel__readonly">
          This session is held by another process.
          <Button size="sm" onClick={onTakeover}>
            Take over
          </Button>
        </div>
      ) : (
        <PromptInput onSubmit={onSubmit} className="chat-composer">
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
