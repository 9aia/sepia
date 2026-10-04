import { useVirtualizer } from "@tanstack/react-virtual";
import { useStickToBottomContext } from "use-stick-to-bottom";
import type { HistoryMessage } from "../lib/types";
import { useHistory } from "../hooks/query/useHistory";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "./ai-elements/conversation";
import { Message, MessageContent, MessageResponse } from "./ai-elements/message";
import { Tool, ToolContent, ToolHeader } from "./ai-elements/tool";

/** Virtualized rows; lives inside Conversation so it can borrow its scroller. */
function HistoryRows({
  history,
  total,
}: {
  readonly history: ReadonlyArray<HistoryMessage>;
  readonly total: number;
}) {
  const { scrollRef } = useStickToBottomContext();
  const virtualizer = useVirtualizer({
    count: history.length,
    getScrollElement: () => scrollRef.current as HTMLElement | null,
    estimateSize: () => 72,
    overscan: 10,
  });

  return (
    <ConversationContent>
      {history.length < total && (
        <div className="history__truncated">
          showing last {history.length} of {total}
        </div>
      )}
      <div
        className="history__rows"
        style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}
      >
        {virtualizer.getVirtualItems().map((row) => {
          const message = history[row.index];
          if (message === undefined) return null;
          return (
            <div
              key={`${message.createdAt}-${row.index}`}
              data-index={row.index}
              ref={virtualizer.measureElement}
              className="history__row"
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${row.start}px)`,
              }}
            >
              {message.toolName !== undefined ? (
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
              ) : (
                <Message from={message.role === "user" ? "user" : "assistant"}>
                  <MessageContent>
                    <MessageResponse>{message.content}</MessageResponse>
                  </MessageContent>
                </Message>
              )}
            </div>
          );
        })}
      </div>
    </ConversationContent>
  );
}

/**
 * Stored backlog for the selected session: a stick-to-bottom conversation
 * with a scroll button, message bubbles and collapsible tool calls.
 */
export function HistoryPane({ sessionId }: { readonly sessionId: string }) {
  const historyQuery = useHistory(sessionId);
  const history = historyQuery.data?.messages ?? [];

  if (history.length === 0) return null;

  return (
    <Conversation className="history">
      <HistoryRows history={history} total={historyQuery.data?.total ?? 0} />
      <ConversationScrollButton />
    </Conversation>
  );
}
