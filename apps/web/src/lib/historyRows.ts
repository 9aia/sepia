import type { HistoryMessage, RunSpan } from "./types";
import type { LiveMessage } from "./liveMessages";
import type { SystemContext } from "./systemContext";

export type ChatRow =
  | { readonly kind: "system"; readonly context: SystemContext }
  | { readonly kind: "history"; readonly message: HistoryMessage }
  | { readonly kind: "span"; readonly label: string; readonly at: number }
  | { readonly kind: "live"; readonly message: LiveMessage };

export type SpanLabel = (span: RunSpan) => string;

const defaultSpanLabel: SpanLabel = (span) => `${span.agent} @ ${span.node}`;

/**
 * Marks which agent+node ran each transcript segment. The first span gets a
 * leading marker so every segment is labeled; each later span's marker sits
 * before the first message at/after its `at` (a span with no messages yet —
 * e.g. the current attach — trails the backlog and labels upcoming live
 * rows). Only rendered once a session has ≥2 spans; below that the single
 * continuous run carries no provenance signal worth a separator.
 */
const insertSpanMarkers = (
  rows: ChatRow[],
  conversation: ReadonlyArray<HistoryMessage>,
  spans: ReadonlyArray<RunSpan>,
  labelFor: SpanLabel,
): void => {
  const ordered = [...spans].sort((a, b) => a.at - b.at);
  const [first, ...rest] = ordered;
  if (first === undefined) return;
  rows.push({ kind: "span", label: labelFor(first), at: first.at });
  let next = 0;
  for (const message of conversation) {
    while (next < rest.length && message.createdAt >= rest[next]!.at) {
      const span = rest[next]!;
      rows.push({ kind: "span", label: labelFor(span), at: span.at });
      next++;
    }
    rows.push({ kind: "history", message });
  }
  for (; next < rest.length; next++) {
    const span = rest[next]!;
    rows.push({ kind: "span", label: labelFor(span), at: span.at });
  }
};

/**
 * All system nodes roll up into one context card at the top — devin emits
 * them per-turn, so positional runs would scatter the cards. Devin also
 * rewrites the context block per internal turn — the same user prompt (and
 * sometimes the reply) lands N times with only system nodes in between —
 * back-to-back duplicates in conversation order collapse.
 */
export const buildRows = (
  history: ReadonlyArray<HistoryMessage>,
  liveMessages: ReadonlyArray<LiveMessage>,
  context: SystemContext,
  spans?: ReadonlyArray<RunSpan>,
  spanLabel: SpanLabel = defaultSpanLabel,
): ChatRow[] => {
  const conversation: HistoryMessage[] = [];
  for (const message of history) {
    if (message.role === "system") continue;
    // Empty assistant nodes carry the turn's tool_calls in the IR — the calls
    // themselves surface as `tool` rows, so a blank bubble is pure noise.
    if (message.role === "assistant" && message.content.trim() === "") continue;
    const prev = conversation[conversation.length - 1];
    if (prev !== undefined && prev.role === message.role && prev.content === message.content) {
      continue;
    }
    conversation.push(message);
  }
  const contextEmpty =
    context.workspaces.length === 0 &&
    context.rules.length === 0 &&
    context.reports.length === 0 &&
    context.promptText === "" &&
    context.platform === null;
  const rows: ChatRow[] = contextEmpty ? [] : [{ kind: "system", context }];
  if (spans !== undefined && spans.length >= 2) {
    insertSpanMarkers(rows, conversation, spans, spanLabel);
  } else {
    for (const message of conversation) rows.push({ kind: "history", message });
  }
  for (const message of liveMessages) rows.push({ kind: "live", message });
  return rows;
};

/**
 * Live rows are optimistic: once the run ends, the refetched IR backlog
 * duplicates them. They should only be cleared once every live user/assistant
 * text is actually present in history — clearing earlier flickers.
 */
export const liveCoveredByHistory = (
  live: ReadonlyArray<LiveMessage>,
  history: ReadonlyArray<HistoryMessage>,
): boolean => {
  if (live.length === 0) return true;
  const historyTexts = new Set(history.map((m) => `${m.role}:${m.content}`));
  return live.every(
    (m) =>
      (m.role !== "user" && m.role !== "assistant") || historyTexts.has(`${m.role}:${m.content}`),
  );
};
