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
import type { RunSpan } from "../lib/types";
import { spanNodeLabel } from "../lib/nodes";
import { cancel, sendPrompt, type StreamStatus } from "../lib/api";
import { sepiaStore, setReplyTo } from "../lib/store";
import { formatReplyPrompt, replyAuthorLabel, type ReplyQuote } from "../lib/reply";
import { settingsStore } from "../lib/settings";
import { buildRows, type ChatRow } from "../lib/historyRows";
import { usePatchSessionMeta } from "../hooks/query/useSessionMeta";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";
import { Skeleton } from "./ui/skeleton";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { BubbleContent } from "./bubble";
import { Message, MessageContent, MessageCopy, MessageFooter, MessageReply } from "./message";
import { flattenHistory, useHistory } from "../hooks/query/useHistory";
import { parseSystemContext, type SystemContext } from "../lib/systemContext";
import { parseErrorPayload, prettifyCode, type ParsedError } from "../lib/errorPayload";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
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
  PromptInputBody,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  type PromptInputMessage,
} from "./prompt-input";

/** Provenance separator — "devin @ thinkpad" — between run segments. */
const spanLabel = (span: RunSpan): string => `${span.agent} @ ${spanNodeLabel(span.node)}`;

function RowContent({ row }: { readonly row: ChatRow }) {
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
        <ToolCall toolName={message.toolName ?? "tool"} done={true} content={message.content} />
      );
    }
    return (
      <MessageRow
        role={message.role === "user" ? "user" : "assistant"}
        content={message.content}
        createdAt={message.createdAt}
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
      createdAt={message.createdAt}
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
}: {
  readonly rows: ReadonlyArray<ChatRow>;
  readonly hasNextPage: boolean;
  readonly fetchingNext: boolean;
  onLoadEarlier: () => void;
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
          <Marker ref={sentinelRef} role={fetchingNext ? "status" : undefined}>
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
                <RowContent row={row} />
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
                  <pre
                    key={i}
                    className="max-h-64 overflow-y-auto rounded-md bg-muted/40 p-2 text-muted-foreground whitespace-pre-wrap"
                  >
                    {report}
                  </pre>
                ))}
              </div>
            </details>
          )}
          {context.promptText !== "" && (
            <details className="group/prompt">
              <summary className="cursor-pointer select-none">
                <ContextSectionLabel icon={File01Icon}>System prompt</ContextSectionLabel>
              </summary>
              <pre className="mt-1.5 max-h-64 overflow-y-auto rounded-md bg-muted/40 p-2 text-muted-foreground whitespace-pre-wrap">
                {context.promptText}
              </pre>
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
  readonly running: boolean;
  readonly streamStatus: StreamStatus;
  readonly liveMessages: ReadonlyArray<LiveMessage>;
  readonly onUserMessage: (text: string) => string;
  readonly onRemoveLiveMessage: (id: string) => void;
  readonly onTakeover: () => void;
}

/**
 * The full conversation for a session: stored IR backlog + live streamed
 * turns in one scroll region, with a PromptInput composer at the bottom.
 */
export function SessionChat({
  sessionId,
  agent,
  readOnly,
  running,
  streamStatus,
  liveMessages,
  onUserMessage,
  onRemoveLiveMessage,
  onTakeover,
}: SessionChatProps) {
  const historyQuery = useHistory(sessionId, agent);
  const history = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  const [submitting, setSubmitting] = useState(false);
  const [promptError, setPromptError] = useState<string | null>(null);
  const replyTo = useStore(sepiaStore, (state) => state.replyTo);
  // Held by another process → the send asks to take over first.
  const [takeoverPrompt, setTakeoverPrompt] = useState<string | null>(null);
  // Populated by ScrollerApiBridge — lets `send` reveal the row it appended.
  const scrollToEnd = useRef<(() => void) | null>(null);
  const scrollOnSent = useRef(false);

  // Run provenance rides on the session row — the meta overlay's spans land
  // in the merged sessions list.
  const { data: sessions = [] } = useSessions();
  const spans = sessions.find((s) => s.id === sessionId && s.agent === agent)?.spans;

  const rows = useMemo<ChatRow[]>(() => {
    const context = parseSystemContext(history.filter((m) => m.role === "system"));
    return buildRows(history, liveMessages, context, spans, spanLabel);
  }, [history, liveMessages, spans]);

  const send = (text: string): void => {
    // Consume any pending reply — the quote rides inside the sent prompt.
    const reply = sepiaStore.state.replyTo;
    if (reply !== null) setReplyTo(null);
    const prompt = reply !== null ? formatReplyPrompt(reply, text) : text;
    setSubmitting(true);
    setPromptError(null);
    // Optimistic — the row shows instantly; rolled back if the send fails.
    const liveId = onUserMessage(prompt);
    // The scroller only trails the bottom while it's already pinned — a send
    // while scrolled up would leave the row appended below the fold (and
    // unmounted by the virtualizer). Flag a scroll for when it commits.
    scrollOnSent.current = true;
    sendPrompt(sessionId, prompt, agent)
      .then((ok) => {
        if (!ok) {
          onRemoveLiveMessage(liveId);
          setPromptError("Prompt failed.");
        }
      })
      .catch((error: unknown) => {
        onRemoveLiveMessage(liveId);
        setPromptError(error instanceof Error ? error.message : "Prompt failed.");
      })
      .finally(() => setSubmitting(false));
  };

  const onSubmit = (message: PromptInputMessage) => {
    const text = message.text.trim();
    if (text === "" || running || submitting) return;
    if (readOnly) {
      setTakeoverPrompt(text);
      return;
    }
    send(text);
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

  // Takeover confirmed → attach resolved readOnly off → send the held text.
  useEffect(() => {
    if (!readOnly && takeoverPrompt !== null) {
      const text = takeoverPrompt;
      setTakeoverPrompt(null);
      send(text);
    }
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

      {promptError !== null && <ErrorBanner>{promptError}</ErrorBanner>}

      <PromptInput onSubmit={onSubmit} className="shrink-0 px-4 pb-4 pt-3">
        <PromptInputBody>
          {replyTo !== null && <ReplyPreview quote={replyTo} />}
          <PromptInputTextarea placeholder="Prompt the agent…" />
        </PromptInputBody>
        <PromptInputFooter>
          <PromptInputTools />
          <div className="ml-auto flex items-center gap-1">
            <ModelSelect sessionId={sessionId} agent={agent} />
            <PromptInputSubmit
              status={running ? "streaming" : submitting ? "submitted" : "ready"}
              disabled={submitting}
              onStop={() => void cancel(sessionId, agent)}
            />
          </div>
        </PromptInputFooter>
      </PromptInput>

      <AlertDialog
        open={takeoverPrompt !== null}
        onOpenChange={(open) => !open && setTakeoverPrompt(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Take over this session?</AlertDialogTitle>
            <AlertDialogDescription>
              This session is held by another process. Taking over detaches it and stops in-progress
              work — your message will be sent after.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onTakeover}>Take over</AlertDialogAction>
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
  const options = [
    ...new Set(
      [
        pref?.model,
        ...(pref?.fallbacks.split(",").map((f) => f.trim()) ?? []),
        session.model,
      ].filter((m): m is string => typeof m === "string" && m !== ""),
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
          patch: { model: v === "__default__" ? null : v },
        })
      }
    >
      <SelectTrigger
        aria-label="Session model"
        title="Model for the next agent spawn"
        className="h-7 w-auto gap-1 border-0 bg-transparent px-2 text-xs text-muted-foreground shadow-none hover:text-foreground"
      >
        {session.model ?? "Default model"}
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

/** ReUI Message anatomy: side-anchored avatar, surface, footer with copy +
 * time. Assistant renders ghost (document-style); user is a tinted bubble
 * aligned to the row's end. */
function MessageRow({
  role,
  content,
  createdAt,
}: {
  readonly role: "user" | "assistant";
  readonly content: string;
  readonly createdAt?: number;
}) {
  // Agents put JSON error payloads in assistant content — a user pasting the
  // same JSON should still render as text.
  const error = role === "assistant" ? parseErrorPayload(content) : null;
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
        </BubbleContent>
        <MessageFooter>
          {role === "assistant" ? (
            <>
              {createdAt !== undefined && <span>{formatMessageTime(createdAt)}</span>}
              <MessageCopy text={() => content} />
              <MessageReply onReply={() => setReplyTo({ role, content, createdAt })} />
            </>
          ) : (
            <>
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
