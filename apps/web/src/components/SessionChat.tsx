import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { AlertCircleIcon, BotIcon } from "@hugeicons/core-free-icons";
import { ArrowDown01Icon } from "@hugeicons/core-free-icons";
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
import type { HistoryMessage } from "../lib/types";
import type { LiveMessage } from "../lib/liveMessages";
import { cancel, sendPrompt } from "../lib/api";
import { settingsStore } from "../lib/settings";
import { usePatchSessionMeta } from "../hooks/query/useSessionMeta";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";
import { Skeleton } from "./ui/skeleton";
import { flattenHistory, useHistory } from "../hooks/query/useHistory";
import { parseSystemContext, type SystemContext } from "../lib/systemContext";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
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
  | { readonly kind: "system"; readonly context: SystemContext }
  | { readonly kind: "live"; readonly message: LiveMessage };

function RowContent({ row }: { readonly row: Row }) {
  if (row.kind === "system") return <SystemContextRow context={row.context} />;
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
    const role = message.role === "user" ? "user" : "assistant";
    const error = parseErrorPayload(message.content);
    return (
      <Message from={role}>
        <div className={`flex flex-col gap-1.5 ${role === "user" ? "items-end" : "items-start"}`}>
          <RowAvatar role={role} />
          <MessageContent>
            {error !== null ? (
              <ErrorMessage error={error} />
            ) : (
              <MessageResponse>{message.content}</MessageResponse>
            )}
          </MessageContent>
        </div>
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
  const role = message.role === "user" ? "user" : "assistant";
  const error = parseErrorPayload(message.content);
  return (
    <Message from={role}>
      <div className={`flex flex-col gap-1.5 ${role === "user" ? "items-end" : "items-start"}`}>
        <RowAvatar role={role} />
        <MessageContent>
          {error !== null ? (
            <ErrorMessage error={error} />
          ) : (
            <MessageResponse>{message.content}</MessageResponse>
          )}
        </MessageContent>
      </div>
    </Message>
  );
}

interface ParsedError {
  readonly code?: string;
  readonly message: string;
}

/** Detect JSON error payloads agents emit as message content. */
function parseErrorPayload(content: string): ParsedError | null {
  const trimmed = content.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as Record<string, unknown>;
    const inner =
      typeof record.error === "object" && record.error !== null
        ? (record.error as Record<string, unknown>)
        : record;
    if (typeof inner.message !== "string") return null;
    return {
      code: typeof inner.code === "string" ? inner.code : undefined,
      message: inner.message,
    };
  } catch {
    return null;
  }
}

const prettifyCode = (code: string): string =>
  code
    .toLowerCase()
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");

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
    <Avatar className="size-6 shrink-0 self-end">
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
  readonly rows: ReadonlyArray<Row>;
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
        {hasNextPage && (
          <div ref={sentinelRef} className="flex justify-center py-2" aria-hidden={!fetchingNext}>
            {fetchingNext && (
              <span className="text-xs text-muted-foreground">Loading earlier…</span>
            )}
          </div>
        )}
        <div className="relative mt-2.5" style={{ height: `${virtualizer.getTotalSize()}px` }}>
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const row = rows[virtualRow.index];
            if (row === undefined) return null;
            const isTurnStart = row.kind !== "system" && row.message.role === "user";
            const key =
              row.kind === "history"
                ? `h-${row.message.createdAt}-${virtualRow.index}`
                : row.kind === "system"
                  ? `sys-${virtualRow.index}`
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
              <span className="font-medium text-foreground/80">Workspace</span>
              {context.workspaces.map((cwd) => (
                <code key={cwd} className="truncate text-muted-foreground">
                  {cwd}
                </code>
              ))}
            </div>
          )}
          {(context.platform !== null || context.osVersion !== null || context.date !== null) && (
            <div className="flex flex-col gap-1">
              <span className="font-medium text-foreground/80">Environment</span>
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
              <span className="font-medium text-foreground/80">Rules</span>
              {context.rules.map((rule) => (
                <code key={`${rule.name}:${rule.path}`} className="truncate text-muted-foreground">
                  {rule.name} — {rule.path}
                </code>
              ))}
            </div>
          )}
          {context.promptText !== "" && (
            <details className="group/prompt">
              <summary className="cursor-pointer font-medium text-foreground/80 select-none">
                System prompt
              </summary>
              <pre className="mt-1.5 max-h-64 overflow-y-auto text-muted-foreground whitespace-pre-wrap">
                {context.promptText}
              </pre>
            </details>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

interface SessionChatProps {
  readonly sessionId: string;
  readonly agent: string;
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
  agent,
  readOnly,
  running,
  liveMessages,
  onUserMessage,
  onTakeover,
}: SessionChatProps) {
  const historyQuery = useHistory(sessionId, agent);
  const history = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  const [submitting, setSubmitting] = useState(false);
  const [promptError, setPromptError] = useState<string | null>(null);
  // Held by another process → the send asks to take over first.
  const [takeoverPrompt, setTakeoverPrompt] = useState<string | null>(null);

  const rows = useMemo<Row[]>(() => {
    // All system nodes roll up into one context card at the top — devin emits
    // them per-turn, so positional runs would scatter the cards.
    const system: HistoryMessage[] = [];
    const conversation: HistoryMessage[] = [];
    for (const message of history) {
      if (message.role === "system") {
        system.push(message);
        continue;
      }
      // Devin rewrites the context block per internal turn — the same user
      // prompt (and sometimes the reply) lands N times with only system nodes
      // in between. Collapse back-to-back duplicates in conversation order.
      const prev = conversation[conversation.length - 1];
      if (prev !== undefined && prev.role === message.role && prev.content === message.content) {
        continue;
      }
      conversation.push(message);
    }
    const context = parseSystemContext(system);
    const empty =
      context.workspaces.length === 0 &&
      context.rules.length === 0 &&
      context.promptText === "" &&
      context.platform === null;
    return [
      ...(empty ? [] : [{ kind: "system", context } satisfies Row]),
      ...conversation.map((message): Row => ({ kind: "history", message })),
      ...liveMessages.map((message): Row => ({ kind: "live", message })),
    ];
  }, [history, liveMessages]);

  const send = (text: string): void => {
    setSubmitting(true);
    setPromptError(null);
    sendPrompt(sessionId, text, agent)
      .then((ok) => {
        if (ok) onUserMessage(text);
        else setPromptError("Prompt failed.");
      })
      .catch((error: unknown) =>
        setPromptError(error instanceof Error ? error.message : "Prompt failed."),
      )
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

  // Takeover confirmed → attach resolved readOnly off → send the held text.
  useEffect(() => {
    if (!readOnly && takeoverPrompt !== null) {
      const text = takeoverPrompt;
      setTakeoverPrompt(null);
      send(text);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readOnly]);

  const loadEarlier = useCallback(() => {
    if (!historyQuery.isFetchingNextPage) void historyQuery.fetchNextPage();
  }, [historyQuery]);

  return (
    <>
      <MessageScrollerProvider autoScroll defaultScrollPosition="end">
        <MessageScroller className="relative flex min-h-0 flex-1 flex-col">
          {rows.length > 0 ? (
            <ChatRows
              rows={rows}
              hasNextPage={historyQuery.hasNextPage}
              fetchingNext={historyQuery.isFetchingNextPage}
              onLoadEarlier={loadEarlier}
            />
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
                    <Skeleton className="h-9 w-2/5 rounded-2xl" />
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

      <PromptInput onSubmit={onSubmit} className="shrink-0 border-t border-border px-4 pb-4 pt-3">
        <PromptInputBody>
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
