import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertCircleIcon,
  BotIcon,
  CheckListIcon,
  ComputerIcon,
  File01Icon,
  Folder01Icon,
  Loading03Icon,
  SourceCodeIcon,
} from "@hugeicons/core-free-icons";
import { ArrowDown01Icon, Cancel01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useUserInfo } from "../hooks/query/useUserInfo";
import { useSessions } from "../hooks/query/useSessions";
import { useStore } from "@tanstack/react-store";
import { Avatar, AvatarFallback } from "./ui/avatar";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./ui/alert-dialog";

import type { LiveMessage } from "../lib/liveMessages";
import type {
  HistoryBlock,
  HistoryMessage,
  MessageUsage,
  PromptCapabilities,
  RunSpan,
} from "../lib/types";
import { attachmentToPart, partToBlock, type PendingAttachment } from "../lib/attachments";
import { attachmentViews } from "../lib/blocks";
import { isModelEnabled } from "../lib/catalog";
import { finishReasonLabel, formatUsage, usageLabel } from "../lib/format";
import { nodeTarget, spanNodeLabel } from "../lib/nodes";
import { ApiError, cancel, sendPrompt, type StreamStatus } from "../lib/api";
import { sepiaStore, setReplyTo } from "../lib/store";
import { formatReplyPrompt, replyAuthorLabel, type ReplyQuote } from "../lib/reply";
import { settingsStore } from "../lib/settings";
import { buildRows, type ChatRow } from "../lib/historyRows";
import { usePatchSessionMeta } from "../hooks/query/useSessionMeta";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";
import { Skeleton } from "./ui/skeleton";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { BubbleContent } from "./bubble";
import {
  Message,
  MessageContent,
  MessageCopy,
  MessageFooter,
  MessageReply,
  MessageRewind,
} from "./message";
import { flattenHistory, useHistory } from "../hooks/query/useHistory";
import { parseSystemContext, type SystemContext } from "../lib/systemContext";
import { parseErrorPayload, prettifyCode, type ParsedError } from "../lib/errorPayload";
import { restoreSummary, useRestoreSession } from "../hooks/query/useRestore";
import { rewindSummary, useRewindSession } from "../hooks/query/useRewind";
import { toastError, toastSuccess } from "../lib/toast";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
import { ScrollArea } from "./ui/scroll-area";
import { ErrorBanner } from "./ErrorBanner";
import { Button } from "./ui/button";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
  useMessageScroller,
} from "./ui/message-scroller";
import { MessageResponse } from "./streamdown";
import { ReasoningBlock } from "./reasoning-block";
import { ToolCall } from "./tool-call";
import {
  PromptInput,
  PromptInputApiBridge,
  PromptInputAttachButton,
  PromptInputAttachments,
  PromptInputBody,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  type PromptInputMessage,
} from "./prompt-input";

/** Provenance separator — "devin @ thinkpad" — between run segments. */
const spanLabel = (span: RunSpan): string => `${span.agent} @ ${spanNodeLabel(span.node)}`;

function RowContent({
  row,
  onRestoreDiff,
  onRewind,
}: {
  readonly row: ChatRow;
  readonly onRestoreDiff?: (path: string, toolCallId?: string) => void;
  /** History rows only — the session can truncate back to this node. */
  readonly onRewind?: (message: HistoryMessage) => void;
}) {
  if (row.kind === "system") return <SystemContextRow context={row.context} />;
  if (row.kind === "span") {
    return (
      <Marker variant="separator">
        <MarkerContent>{row.label}</MarkerContent>
      </Marker>
    );
  }
  if (row.kind === "history") {
    const message = row.message;
    if (message.role === "tool") {
      return (
        <ToolCall
          toolName={message.toolName ?? "tool"}
          done={true}
          args={message.args}
          content={message.content}
          status={message.toolStatus}
          exitCode={message.exitCode}
          durationMs={message.durationMs}
          diffs={message.diffs}
          locations={message.locations}
          toolCallId={message.toolCallId}
          onRestoreDiff={onRestoreDiff}
        />
      );
    }
    return (
      <MessageRow
        role={message.role === "user" ? "user" : "assistant"}
        content={message.content}
        blocks={message.blocks}
        createdAt={message.createdAt}
        usage={message.usage}
        finishReason={message.finishReason}
        onRewind={
          onRewind !== undefined && message.nodeId !== undefined
            ? () => onRewind(message)
            : undefined
        }
      />
    );
  }

  const message = row.message;
  if (message.role === "tool") {
    return (
      <ToolCall
        toolName={message.toolName ?? "tool"}
        done={message.done}
        args={message.args}
        content={message.content}
        status={message.toolStatus}
        exitCode={message.exitCode}
        durationMs={message.durationMs}
        diffs={message.diffs}
        locations={message.locations}
        contents={message.contents}
        onRestoreDiff={onRestoreDiff}
      />
    );
  }
  if (message.role === "status") {
    return (
      <Marker variant="separator">
        <MarkerContent>{message.content}</MarkerContent>
      </Marker>
    );
  }
  if (message.role === "reasoning") {
    return <ReasoningBlock done={message.done} content={message.content} />;
  }
  return (
    <MessageRow
      role={message.role === "user" ? "user" : "assistant"}
      content={message.content}
      blocks={message.blocks}
      createdAt={message.createdAt}
      usage={message.usage}
      finishReason={message.finishReason}
    />
  );
}

function ErrorMessage({ error }: { readonly error: ParsedError }) {
  return (
    <div
      role="alert"
      className="flex items-start gap-2.5 rounded-xl border border-destructive/40 bg-destructive/10 px-3.5 py-2.5"
    >
      <HugeiconsIcon icon={AlertCircleIcon} className="mt-0.5 size-4 shrink-0 text-destructive" />
      <div className="min-w-0">
        <div className="text-sm font-medium">
          {error.code !== undefined ? prettifyCode(error.code) : "Something went wrong"}
        </div>
        <p className="m-0 text-xs text-muted-foreground">{error.message}</p>
      </div>
    </div>
  );
}

function RowAvatar({ role }: { readonly role: string }) {
  const { data: user } = useUserInfo();
  return (
    <Avatar className="size-6 shrink-0">
      <AvatarFallback className="text-[10px]">
        {role === "user" ? (
          (user?.username.charAt(0).toUpperCase() ?? "?")
        ) : (
          <HugeiconsIcon icon={BotIcon} className="size-3.5" />
        )}
      </AvatarFallback>
    </Avatar>
  );
}

/** Virtualized rows inside the message-scroller viewport. */
function ChatRows({
  rows,
  hasNextPage,
  fetchingNext,
  onLoadEarlier,
  onRestoreDiff,
  onRewind,
}: {
  readonly rows: ReadonlyArray<ChatRow>;
  readonly hasNextPage: boolean;
  readonly fetchingNext: boolean;
  onLoadEarlier: () => void;
  readonly onRestoreDiff?: (path: string, toolCallId?: string) => void;
  readonly onRewind?: (message: HistoryMessage) => void;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => 72,
    overscan: 10,
  });

  // Scroll-to-top pagination: the sentinel at the top of the content triggers
  // the next page; preserveScrollOnPrepend keeps the reader's position.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    const root = viewportRef.current;
    if (sentinel === null || root === null || !hasNextPage) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) onLoadEarlier();
      },
      { root, rootMargin: "80px" },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasNextPage, onLoadEarlier]);

  return (
    <MessageScrollerViewport ref={viewportRef}>
      <MessageScrollerContent className="mx-auto w-full max-w-3xl px-4">
        {hasNextPage ? (
          <Marker
            ref={sentinelRef}
            variant={fetchingNext ? "border" : "default"}
            role={fetchingNext ? "status" : undefined}
          >
            {fetchingNext && (
              <>
                <MarkerIcon>
                  <HugeiconsIcon icon={Loading03Icon} className="animate-spin" />
                </MarkerIcon>
                <MarkerContent>Loading earlier…</MarkerContent>
              </>
            )}
          </Marker>
        ) : (
          rows.length > 0 && (
            <Marker variant="separator">
              <MarkerContent>Start of session</MarkerContent>
            </Marker>
          )
        )}
        <div className="relative mt-2.5" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const row = rows[virtualRow.index];
            if (row === undefined) return null;
            const isTurnStart =
              (row.kind === "history" || row.kind === "live") && row.message.role === "user";
            const key =
              row.kind === "history"
                ? `h-${row.message.createdAt}-${virtualRow.index}`
                : row.kind === "system"
                  ? `sys-${virtualRow.index}`
                  : row.kind === "span"
                    ? `span-${row.at}-${virtualRow.index}`
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
                <RowContent row={row} onRestoreDiff={onRestoreDiff} onRewind={onRewind} />
              </MessageScrollerItem>
            );
          })}
        </div>
      </MessageScrollerContent>
    </MessageScrollerViewport>
  );
}

/** Section heading inside the context card — icon + label. */
function ContextSectionLabel({
  icon,
  children,
}: {
  readonly icon: typeof Folder01Icon;
  readonly children: ReactNode;
}) {
  return (
    <span className="inline-flex items-center gap-1.5 font-medium text-foreground/80">
      <HugeiconsIcon icon={icon} className="size-3.5 text-muted-foreground" strokeWidth={2} />
      {children}
    </span>
  );
}

/** The agent's stored system prompt, parsed into a compact context card. */
function SystemContextRow({ context }: { readonly context: SystemContext }) {
  const summary = [context.workspaces[0], context.platform, context.osVersion, context.date]
    .filter((part): part is string => part !== null)
    .join(" · ");
  return (
    <Collapsible className="rounded-lg border border-border/60 bg-muted/30">
      <CollapsibleTrigger className="group flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-muted-foreground transition-colors hover:text-foreground">
        <span className="font-medium text-foreground/80">Session context</span>
        <span className="min-w-0 flex-1 truncate">
          {summary !== ""
            ? summary
            : context.rules.length > 0
              ? `${context.rules.length} rule${context.rules.length === 1 ? "" : "s"}`
              : "System prompt"}
        </span>
        <HugeiconsIcon
          icon={ArrowDown01Icon}
          strokeWidth={2}
          className="size-3.5 shrink-0 transition-transform group-data-[panel-open]:rotate-180"
        />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col gap-3 border-t border-border/60 px-3 py-2.5 text-xs">
          {context.workspaces.length > 0 && (
            <div className="flex flex-col gap-1">
              <ContextSectionLabel icon={Folder01Icon}>Workspace</ContextSectionLabel>
              {context.workspaces.map((cwd) => (
                <code key={cwd} className="truncate text-muted-foreground" title={cwd}>
                  {cwd}
                </code>
              ))}
            </div>
          )}
          {(context.platform !== null || context.osVersion !== null || context.date !== null) && (
            <div className="flex flex-col gap-1">
              <ContextSectionLabel icon={ComputerIcon}>Environment</ContextSectionLabel>
              <span className="text-muted-foreground">
                {[
                  context.platform !== null ? `Platform: ${context.platform}` : null,
                  context.osVersion !== null ? `OS: ${context.osVersion}` : null,
                  context.date !== null ? `Date: ${context.date}` : null,
                ]
                  .filter((line): line is string => line !== null)
                  .join(" · ")}
              </span>
            </div>
          )}
          {context.rules.length > 0 && (
            <div className="flex flex-col gap-1">
              <ContextSectionLabel icon={CheckListIcon}>Rules</ContextSectionLabel>
              {context.rules.map((rule) => (
                <div key={`${rule.name}:${rule.path}`} className="flex min-w-0 gap-1.5">
                  <span className="shrink-0 text-foreground/70">{rule.name}</span>
                  <code className="truncate text-muted-foreground/80" title={rule.path}>
                    {rule.path}
                  </code>
                </div>
              ))}
            </div>
          )}
          {context.reports.length > 0 && (
            <details className="group/reports">
              <summary className="cursor-pointer select-none">
                <ContextSectionLabel icon={SourceCodeIcon}>
                  {`Background agent reports (${context.reports.length})`}
                </ContextSectionLabel>
              </summary>
              <div className="mt-1.5 flex flex-col gap-2">
                {context.reports.map((report, i) => (
                  <div key={i} className="rounded-md bg-muted/40">
                    {(report.title !== undefined ||
                      report.agentId !== undefined ||
                      report.at !== undefined) && (
                      <div className="flex min-w-0 items-center gap-2 px-2 pt-1.5 text-[11px]">
                        {report.title !== undefined && (
                          <span
                            className="min-w-0 flex-1 truncate font-medium text-foreground/70"
                            title={report.title}
                          >
                            {report.title}
                          </span>
                        )}
                        {report.agentId !== undefined && (
                          <code className="shrink-0 text-muted-foreground/80">
                            {`agent ${report.agentId}`}
                          </code>
                        )}
                        {report.at !== undefined && (
                          <span className="shrink-0 text-muted-foreground/80">
                            {formatMessageTime(report.at)}
                          </span>
                        )}
                      </div>
                    )}
                    <ScrollArea className="max-h-64">
                      <pre className="p-2 text-muted-foreground whitespace-pre-wrap">
                        {report.body}
                      </pre>
                    </ScrollArea>
                  </div>
                ))}
              </div>
            </details>
          )}
          {context.promptText !== "" && (
            <details className="group/prompt">
              <summary className="cursor-pointer select-none">
                <ContextSectionLabel icon={File01Icon}>System prompt</ContextSectionLabel>
              </summary>
              <ScrollArea className="mt-1.5 max-h-64 rounded-md bg-muted/40">
                <pre className="p-2 text-muted-foreground whitespace-pre-wrap">
                  {context.promptText}
                </pre>
              </ScrollArea>
            </details>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/**
 * `useMessageScroller` only works under the Provider, but the send path lives
 * in SessionChat — this bridge hands `scrollToEnd` out through a ref.
 */
function ScrollerApiBridge({ apiRef }: { readonly apiRef: RefObject<(() => void) | null> }) {
  const { scrollToEnd } = useMessageScroller();
  useEffect(() => {
    apiRef.current = () => scrollToEnd({ behavior: "auto" });
    return () => {
      apiRef.current = null;
    };
  }, [apiRef, scrollToEnd]);
  return null;
}

interface SessionChatProps {
  readonly sessionId: string;
  readonly agent: string;
  readonly readOnly: boolean;
  /**
   * The attached agent's advertised prompt capabilities (from the attach
   * probe). `undefined` = unprobed/older peer — everything stays enabled.
   */
  readonly promptCapabilities?: PromptCapabilities;
  readonly running: boolean;
  readonly streamStatus: StreamStatus;
  readonly liveMessages: ReadonlyArray<LiveMessage>;
  readonly onUserMessage: (text: string, blocks?: ReadonlyArray<HistoryBlock>) => string;
  readonly onRemoveLiveMessage: (id: string) => void;
  /** Last takeover attempt's failure — shown inside the dialog, which stays open. */
  readonly takeoverError: string | null;
  /** A takeover attach is in flight — the dialog shows progress, not a dead button. */
  readonly takeoverPending: boolean;
  /** Pid of the process holding the session, when the agent reported one. */
  readonly holderPid: number | null;
  readonly onTakeover: () => void;
  /** Plain re-attach (no takeover) — used to retry a send that 400'd "not attached". */
  readonly onReattach: () => Promise<boolean>;
}

/**
 * The full conversation for a session: stored IR backlog + live streamed
 * turns in one scroll region, with a PromptInput composer at the bottom.
 */
export function SessionChat({
  sessionId,
  agent,
  readOnly,
  promptCapabilities,
  running,
  streamStatus,
  liveMessages,
  onUserMessage,
  onRemoveLiveMessage,
  takeoverError,
  takeoverPending,
  holderPid,
  onTakeover,
  onReattach,
}: SessionChatProps) {
  // Run provenance + node routing ride on the session row — the meta
  // overlay's spans land in the merged sessions list, and `node` scopes
  // history/prompt/cancel to the owning machine (direct or via gateway).
  const { data: sessions = [] } = useSessions();
  const sessionRow = sessions.find((s) => s.id === sessionId && s.agent === agent);
  const historyQuery = useHistory(sessionId, agent, sessionRow?.node);
  const history = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  const [submitting, setSubmitting] = useState(false);
  // The failed send's payload rides along so Retry can resend it verbatim.
  const [promptError, setPromptError] = useState<{
    message: string;
    /** Server answered "not attached" — retry must re-attach first. */
    notAttached: boolean;
    resend: { text: string; attachments: PendingAttachment[] };
  } | null>(null);
  const replyTo = useStore(sepiaStore, (state) => state.replyTo);
  // Held by another process → the send queues behind a takeover confirm. The
  // sessionId guards the flush: a queue raised on one session must never fire
  // into another after a switch.
  const [takeoverPrompt, setTakeoverPrompt] = useState<{
    sessionId: string;
    text: string;
    attachments: PendingAttachment[];
  } | null>(null);
  // A per-file restore requested from a tool-call diff row — the dialog
  // confirms before the server writes the file back.
  const [restoreTarget, setRestoreTarget] = useState<{
    path: string;
    toolCallId?: string;
  } | null>(null);
  const restore = useRestoreSession();
  // A conversation rewind requested from a history row — the dialog
  // confirms before the server truncates the transcript at that node.
  const [rewindTarget, setRewindTarget] = useState<HistoryMessage | null>(null);
  const rewind = useRewindSession();
  // Populated by ScrollerApiBridge — lets `send` reveal the row it appended.
  const scrollToEnd = useRef<(() => void) | null>(null);
  const scrollOnSent = useRef(false);
  // Composer access for the held-session queue: the textarea ref flushes the
  // box after the deferred send lands. Populated by PromptInputApiBridge —
  // clears the attachment tray the same way (a queued submit keeps its chips
  // until the send actually happens).
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const clearAttachmentsRef = useRef<(() => void) | null>(null);

  const spans = sessionRow?.spans;

  const rows = useMemo<ChatRow[]>(() => {
    // Devin emits system nodes per-turn — parsing them from a partial window
    // renders the context card relative to loaded pages instead of the
    // conversation's absolute beginning. Only build it once pagination has
    // reached the start (hasNextPage false ⇒ the earliest page landed).
    const context = parseSystemContext(
      historyQuery.hasNextPage === false ? history.filter((m) => m.role === "system") : [],
    );
    return buildRows(history, liveMessages, context, spans, spanLabel);
  }, [history, liveMessages, spans, historyQuery.hasNextPage]);

  const send = (text: string, attachments: ReadonlyArray<PendingAttachment> = []): void => {
    // Consume any pending reply — the quote rides inside the sent prompt.
    const reply = sepiaStore.state.replyTo;
    if (reply !== null) setReplyTo(null);
    const prompt = reply !== null ? formatReplyPrompt(reply, text) : text;
    const parts = attachments.map(attachmentToPart);
    setSubmitting(true);
    setPromptError(null);
    // Optimistic — the row shows instantly; rolled back if the send fails.
    // The blocks mirror what the flushed IR carries, so attachments render
    // in the live row exactly as history will show them.
    const liveId = onUserMessage(prompt, [
      // The text part only goes on the wire when non-empty — same here, so the
      // optimistic blocks match what the flushed IR records.
      ...(prompt !== "" ? ([{ type: "text", text: prompt }] as const) : []),
      ...parts.map(partToBlock),
    ]);
    // The scroller only trails the bottom while it's already pinned — a send
    // while scrolled up would leave the row appended below the fold (and
    // unmounted by the virtualizer). Flag a scroll for when it commits.
    scrollOnSent.current = true;
    sendPrompt(sessionId, { text: prompt, attachments: parts }, agent, nodeTarget(sessionRow?.node))
      .then((ok) => {
        if (!ok) {
          onRemoveLiveMessage(liveId);
          setPromptError({
            message: "Prompt failed.",
            notAttached: false,
            resend: { text: prompt, attachments: [...attachments] },
          });
        }
      })
      .catch((error: unknown) => {
        onRemoveLiveMessage(liveId);
        // A 400 "Session is not attached" means the live agent went away
        // (idle detach, server restart) — retry goes through re-attach first.
        const notAttached =
          error instanceof ApiError && error.status === 400 && /not attached/i.test(error.message);
        setPromptError({
          message: notAttached
            ? "The session isn't attached anymore — re-attach and retry."
            : error instanceof Error
              ? error.message
              : "Prompt failed.",
          notAttached,
          resend: { text: prompt, attachments: [...attachments] },
        });
      })
      .finally(() => setSubmitting(false));
  };

  const retryPrompt = (): void => {
    const failure = promptError;
    if (failure === null) return;
    setPromptError(null);
    if (!failure.notAttached) {
      send(failure.resend.text, failure.resend.attachments);
      return;
    }
    // The prompt payload only sends once the session is attached again.
    setSubmitting(true);
    onReattach()
      .then((ok) => {
        if (ok) {
          send(failure.resend.text, failure.resend.attachments);
        } else {
          // Attached read-only — the held banner offers the takeover path.
          setPromptError({
            ...failure,
            message: "The session is still held — take it over to send.",
          });
        }
      })
      .catch((error: unknown) => {
        setPromptError({
          ...failure,
          message: error instanceof Error ? error.message : "Re-attach failed.",
        });
      })
      .finally(() => setSubmitting(false));
  };

  const onSubmit = (message: PromptInputMessage): false | void => {
    const text = message.text.trim();
    const empty = text === "" && message.attachments.length === 0;
    // Held by another process — the send queues behind a takeover confirm.
    // Returning `false` keeps the draft in the composer: it's the queued
    // payload, left in place for editing on cancel and for the retry when a
    // takeover fails.
    if (readOnly) {
      if (!empty) {
        setTakeoverPrompt({ sessionId, text, attachments: message.attachments });
      }
      return false;
    }
    // A blocked submit keeps the draft too — Enter during a run or an
    // in-flight send must not drop what was just typed.
    if (empty || running || submitting) return false;
    send(text, message.attachments);
  };

  // Reveal the just-sent row once it's committed — a second pass after a
  // frame covers the virtualizer measuring it taller than the estimate.
  useEffect(() => {
    if (!scrollOnSent.current) return;
    scrollOnSent.current = false;
    scrollToEnd.current?.();
    const frame = requestAnimationFrame(() => scrollToEnd.current?.());
    return () => cancelAnimationFrame(frame);
  }, [liveMessages]);

  // A queue or confirm raised on another session is stale — drop it on
  // switch so the dialog can't fire the old text into the new session.
  useEffect(() => {
    setTakeoverPrompt(null);
  }, [sessionId]);

  // Takeover confirmed → attach resolved readOnly off → close the dialog and
  // send the held prompt. A failed takeover keeps readOnly on, so neither the
  // dialog nor the queued message is dropped.
  useEffect(() => {
    if (readOnly) return;
    if (takeoverPrompt === null) return;
    const held = takeoverPrompt;
    setTakeoverPrompt(null);
    if (held.sessionId !== sessionId) return;
    // The composer kept the draft on submit (`false`) — the send below is
    // what consumed it, so the box and tray flush now like a normal send.
    textareaRef.current?.form?.reset();
    clearAttachmentsRef.current?.();
    send(held.text, held.attachments);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readOnly]);

  // Throttled — the sentinel refires while visible, so a dead server would
  // otherwise get hammered by fetchNextPage retries.
  const lastEarlier = useRef(0);
  const loadEarlier = useCallback(() => {
    const now = Date.now();
    if (historyQuery.isFetchingNextPage || now - lastEarlier.current < 1500) return;
    lastEarlier.current = now;
    void historyQuery.fetchNextPage();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [historyQuery.isFetchingNextPage, historyQuery.fetchNextPage]);

  return (
    <>
      <MessageScrollerProvider autoScroll defaultScrollPosition="end" scrollEdgeThreshold={8}>
        <ScrollerApiBridge apiRef={scrollToEnd} />
        <MessageScroller className="relative flex min-h-0 flex-1 flex-col">
          {streamStatus === "reconnecting" && (
            <div className="pointer-events-none absolute inset-x-0 top-3 z-10 flex justify-center">
              <span className="flex items-center gap-2 rounded-full border border-border bg-popover/95 px-3 py-1.5 text-xs text-muted-foreground shadow-lg backdrop-blur-sm">
                <HugeiconsIcon
                  icon={Loading03Icon}
                  className="size-3.5 animate-spin"
                  strokeWidth={2}
                />
                Reconnecting…
              </span>
            </div>
          )}
          {rows.length > 0 ? (
            <>
              <ChatRows
                rows={rows}
                hasNextPage={historyQuery.hasNextPage}
                fetchingNext={historyQuery.isFetchingNextPage}
                onLoadEarlier={loadEarlier}
                onRestoreDiff={(path, toolCallId) => setRestoreTarget({ path, toolCallId })}
                onRewind={readOnly || running ? undefined : (message) => setRewindTarget(message)}
              />
              {running && !liveMessages.some((m) => !m.done) && (
                <div className="mx-auto flex w-full max-w-3xl items-center gap-2 px-4 py-3 text-sm">
                  <RowAvatar role="assistant" />
                  <span className="shimmer-text">Thinking…</span>
                </div>
              )}
            </>
          ) : (
            <MessageScrollerViewport>
              {historyQuery.isLoading ? (
                <div className="mx-auto flex w-full max-w-3xl flex-col justify-end gap-5 p-4">
                  <div className="flex flex-col items-start gap-1.5">
                    <Skeleton className="size-7 rounded-full" />
                    <Skeleton className="h-10 w-3/5 rounded-2xl" />
                  </div>
                  <div className="flex flex-col items-end gap-1.5">
                    <Skeleton className="size-7 rounded-full" />
                    <Skeleton className="h-9 w-2/5 rounded-lg" />
                  </div>
                </div>
              ) : (
                <p className="mx-auto w-full max-w-3xl p-4 text-muted-foreground">
                  No messages yet.
                </p>
              )}
            </MessageScrollerViewport>
          )}
          <MessageScrollerButton direction="end" />
        </MessageScroller>
      </MessageScrollerProvider>

      {promptError !== null && (
        <ErrorBanner
          action={
            <>
              <Button size="xs" variant="secondary" disabled={submitting} onClick={retryPrompt}>
                {promptError.notAttached ? "Re-attach & retry" : "Retry"}
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label="Dismiss error"
                onClick={() => setPromptError(null)}
              >
                <HugeiconsIcon icon={Cancel01Icon} />
              </Button>
            </>
          }
        >
          {promptError.message}
        </ErrorBanner>
      )}

      {/* env() resolves to 0 outside notched devices — the max() keeps the
          1rem padding everywhere else, so desktop is unchanged. */}
      <PromptInput
        onSubmit={onSubmit}
        className="shrink-0 px-4 pt-3 pb-[max(1rem,env(safe-area-inset-bottom))]"
      >
        <PromptInputApiBridge apiRef={clearAttachmentsRef} />
        <PromptInputBody>
          {replyTo !== null && <ReplyPreview quote={replyTo} />}
          <PromptInputAttachments />
          {/* Held by another process: the composer looks and stays live —
              a real submit queues behind the takeover confirm dialog; the
              draft is left in place for editing on cancel or retry. */}
          <PromptInputTextarea ref={textareaRef} placeholder="Prompt the agent…" />
        </PromptInputBody>
        <PromptInputFooter>
          <PromptInputTools>
            {/* Hidden when the agent advertised it can't take images — the
                button would just produce a send-time rejection. */}
            {promptCapabilities?.image !== false && <PromptInputAttachButton />}
          </PromptInputTools>
          <div className="ml-auto flex min-w-0 items-center gap-1">
            <ModelSelect sessionId={sessionId} agent={agent} />
            <PromptInputSubmit
              status={running ? "streaming" : submitting ? "submitted" : "ready"}
              disabled={submitting}
              onStop={() => void cancel(sessionId, agent, nodeTarget(sessionRow?.node))}
            />
          </div>
        </PromptInputFooter>
      </PromptInput>

      <AlertDialog
        open={takeoverPrompt !== null}
        onOpenChange={(open) => {
          if (open) return;
          // Cancel drops the queue, not the draft — it never left the
          // composer, so the text and chips are still there to edit or retry.
          setTakeoverPrompt(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Take over this session?</AlertDialogTitle>
            <AlertDialogDescription>
              {holderPid !== null
                ? `Sending will stop the run on the other process (PID ${holderPid}) and hand control to you — your message will be sent after.`
                : "Sending will stop the run on the other process and hand control to you — your message will be sent after."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {takeoverError !== null && (
            <p role="alert" className="m-0 text-sm text-destructive">
              {takeoverError}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={takeoverPending}>Cancel</AlertDialogCancel>
            {/* preventDefault keeps the dialog open — a failed takeover
                reports in place (with Try again), a successful one closes
                when readOnly flips off. */}
            <AlertDialogAction
              disabled={takeoverPending}
              onClick={(event) => {
                event.preventDefault();
                onTakeover();
              }}
            >
              {takeoverPending
                ? "Taking over…"
                : takeoverError !== null
                  ? "Try again"
                  : "Take over"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* Per-file restore: the server reverse-applies the recorded diff —
          skips land in the toast rather than clobbering drifted files. */}
      <AlertDialog
        open={restoreTarget !== null}
        onOpenChange={(open) => {
          if (!open && !restore.isPending) setRestoreTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Restore this file?</AlertDialogTitle>
            <AlertDialogDescription>
              Write <code className="break-all">{restoreTarget?.path}</code> back to its state
              before this change. Current edits to the file are overwritten.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {restore.isError && (
            <p role="alert" className="m-0 text-sm text-destructive">
              {restore.error instanceof Error ? restore.error.message : "Restore failed"}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={restore.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={restore.isPending}
              onClick={(event) => {
                event.preventDefault();
                const target = restoreTarget;
                if (target === null) return;
                restore.mutate(
                  {
                    sessionId,
                    agent,
                    node: sessionRow?.node,
                    selector: {
                      path: target.path,
                      ...(target.toolCallId === undefined ? {} : { toolCallId: target.toolCallId }),
                    },
                  },
                  {
                    onSuccess: (result) => {
                      setRestoreTarget(null);
                      toastSuccess(restoreSummary(result), undefined);
                      if (result.skipped.length > 0) {
                        toastError(
                          "Some files were skipped",
                          new Error(result.skipped.map((s) => `${s.path}: ${s.reason}`).join("\n")),
                        );
                      }
                    },
                  },
                );
              }}
            >
              {restore.isPending ? "Restoring…" : "Restore"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* Conversation rewind: the server truncates the transcript after
          this message — everything past it is deleted, not just hidden. */}
      <AlertDialog
        open={rewindTarget !== null}
        onOpenChange={(open) => {
          if (!open && !rewind.isPending) setRewindTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rewind to this message?</AlertDialogTitle>
            <AlertDialogDescription>
              The conversation will end here — every message after this one is deleted from the
              session's history. Files the agent already changed are not touched (use restore for
              that). This can't be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {rewind.isError && (
            <p role="alert" className="m-0 text-sm text-destructive">
              {rewind.error instanceof Error ? rewind.error.message : "Rewind failed"}
            </p>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={rewind.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={rewind.isPending}
              onClick={(event) => {
                event.preventDefault();
                const target = rewindTarget;
                if (target === null || target.nodeId === undefined) return;
                rewind.mutate(
                  {
                    sessionId,
                    agent,
                    node: sessionRow?.node,
                    selector: { nodeId: target.nodeId },
                  },
                  {
                    onSuccess: (result) => {
                      setRewindTarget(null);
                      toastSuccess(rewindSummary(result), undefined);
                    },
                  },
                );
              }}
            >
              {rewind.isPending ? "Rewinding…" : "Rewind"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/** Per-session model selector — the choice applies on the next agent spawn. */
function ModelSelect({ sessionId, agent }: { readonly sessionId: string; readonly agent: string }) {
  const patch = usePatchSessionMeta();
  const settings = useStore(settingsStore);
  const { data: sessions = [] } = useSessions();
  const session = sessions.find((s) => s.id === sessionId && s.agent === agent);
  if (session === undefined) return null;
  const pref = settings.models[session.agent];
  // Parked models drop out of the picker — except the session's own stored
  // pick, which stays visible (it just won't apply at the next spawn).
  const options = [
    ...new Set(
      [
        pref?.model,
        ...(pref?.fallbacks.split(",").map((f) => f.trim()) ?? []),
        session.model,
      ].filter(
        (m): m is string =>
          typeof m === "string" &&
          m !== "" &&
          (m === session.model || isModelEnabled(settings, session.node, session.agent, m)),
      ),
    ),
  ];
  const value = session.model ?? "__default__";
  return (
    <Select
      value={value}
      onValueChange={(v) =>
        patch.mutate({
          id: session.id,
          agent: session.agent,
          node: session.node,
          patch: { model: v === "__default__" ? null : v },
        })
      }
    >
      <SelectTrigger
        aria-label="Session model"
        title="Model for the next agent spawn"
        // max-w keeps a long model id from pushing the send button out of
        // the composer on narrow viewports.
        className="h-7 w-auto max-w-40 gap-1 border-0 bg-transparent px-2 text-xs text-muted-foreground shadow-none hover:text-foreground"
      >
        <span className="min-w-0 truncate">{session.model ?? "Default model"}</span>
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="__default__">Agent default</SelectItem>
        {options.map((model) => (
          <SelectItem key={model} value={model}>
            {model}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

const formatMessageTime = (ms: number): string =>
  new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

/** Non-text content blocks — inline images and file chips under the text. */
function Attachments({ blocks }: { readonly blocks: ReadonlyArray<HistoryBlock> }) {
  const views = attachmentViews(blocks);
  if (views.length === 0) return null;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2">
      {views.map((view, i) =>
        view.kind === "image" ? (
          <img
            key={i}
            src={view.src}
            alt={view.alt}
            loading="lazy"
            className="max-h-48 max-w-full rounded-lg border border-border/60 object-contain"
          />
        ) : (
          <span
            key={i}
            title={view.name}
            className="inline-flex max-w-full items-center gap-1.5 rounded-md border border-border/60 bg-background/40 px-2 py-1 text-xs text-muted-foreground"
          >
            <HugeiconsIcon icon={File01Icon} className="size-3.5 shrink-0" strokeWidth={2} />
            <span className="min-w-0 truncate">{view.name}</span>
            {view.detail !== "" && (
              <span className="shrink-0 text-muted-foreground/60">{view.detail}</span>
            )}
          </span>
        ),
      )}
    </div>
  );
}

/** ReUI Message anatomy: side-anchored avatar, surface, footer with copy +
 * time. Assistant renders ghost (document-style); user is a tinted bubble
 * aligned to the row's end. */
function MessageRow({
  role,
  content,
  blocks,
  createdAt,
  usage,
  finishReason,
  onRewind,
}: {
  readonly role: "user" | "assistant";
  readonly content: string;
  /** Content blocks the store recorded — attachments render under the text. */
  readonly blocks?: ReadonlyArray<HistoryBlock>;
  readonly createdAt?: number;
  /** IR v2 token metrics — renders a compact ↑in ↓out in the footer. */
  readonly usage?: MessageUsage;
  /** Non-"stop" endings get a tiny marker; quiet endings render nothing. */
  readonly finishReason?: string;
  /** History rows only — truncate the session back to this message. */
  readonly onRewind?: () => void;
}) {
  // Agents put JSON error payloads in assistant content — a user pasting the
  // same JSON should still render as text.
  const error = role === "assistant" ? parseErrorPayload(content) : null;
  const finish = finishReasonLabel(finishReason);
  return (
    <Message align={role === "user" ? "end" : "start"}>
      <MessageContent>
        <RowAvatar role={role} />
        <BubbleContent
          variant={role === "user" ? "default" : "ghost"}
          className={role === "user" ? "max-w-[80%]" : "max-w-none"}
        >
          {error !== null ? (
            <ErrorMessage error={error} />
          ) : (
            <MessageResponse>{content}</MessageResponse>
          )}
          {blocks !== undefined && <Attachments blocks={blocks} />}
        </BubbleContent>
        <MessageFooter>
          {role === "assistant" ? (
            <>
              {createdAt !== undefined && <span>{formatMessageTime(createdAt)}</span>}
              {usage !== undefined && (
                <span className="text-muted-foreground/80" title={formatUsage(usage)}>
                  {usageLabel(usage)}
                </span>
              )}
              {finish !== null && (
                <span
                  className={finish === "error" ? "text-destructive" : "text-muted-foreground/80"}
                  title={`Finish reason: ${finish}`}
                >
                  {finish}
                </span>
              )}
              <MessageCopy text={() => content} />
              <MessageReply onReply={() => setReplyTo({ role, content, createdAt })} />
              {onRewind !== undefined && <MessageRewind onRewind={onRewind} />}
            </>
          ) : (
            <>
              {onRewind !== undefined && <MessageRewind onRewind={onRewind} />}
              <MessageReply onReply={() => setReplyTo({ role, content, createdAt })} />
              <MessageCopy text={() => content} />
              {createdAt !== undefined && <span>{formatMessageTime(createdAt)}</span>}
            </>
          )}
        </MessageFooter>
      </MessageContent>
    </Message>
  );
}

/** Compact quote card shown inside the composer while a reply is pending. */
function ReplyPreview({ quote }: { readonly quote: ReplyQuote }) {
  return (
    <div className="mx-3 mt-2 flex items-start gap-1 border-l-2 border-primary/50 pl-2">
      <div className="min-w-0 flex-1">
        <div className="text-xs font-medium text-foreground/80">{replyAuthorLabel(quote.role)}</div>
        <p className="m-0 line-clamp-2 text-xs whitespace-pre-wrap text-muted-foreground">
          {quote.content}
        </p>
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Dismiss reply"
        title="Dismiss reply"
        onClick={() => setReplyTo(null)}
      >
        <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
      </Button>
    </div>
  );
}
