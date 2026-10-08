import type { HistoryMessage, RunSpan } from "./types";
import { hasAttachments } from "./blocks";
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
 * Marks which agent+node ran each transcript segment. A span at/before the
 * first message heads the transcript — its marker goes on top so every
 * segment is labeled; each later span's marker sits before the first
 * message at/after its `at` (a span with no messages yet — e.g. the current
 * attach — trails the backlog and labels upcoming live rows). A span that
 * starts mid-transcript — a transfer-in — marks inline at its boundary even
 * when it's the only span recorded. The one case with no marker: a single
 * span covering the whole transcript — the session ran end-to-end on one
 * node+agent, which the header already states.
 */
const insertSpanMarkers = (
  rows: ChatRow[],
  conversation: ReadonlyArray<HistoryMessage>,
  spans: ReadonlyArray<RunSpan>,
  labelFor: SpanLabel,
): void => {
  const ordered = [...spans].sort((a, b) => a.at - b.at);
  const firstAt = conversation[0]?.createdAt;
  const headsAll = ordered.length > 0 && (firstAt === undefined || ordered[0]!.at <= firstAt);
  if (headsAll && ordered.length === 1 && conversation.length > 0) {
    for (const message of conversation) rows.push({ kind: "history", message });
    return;
  }
  const inline = headsAll ? ordered.slice(1) : ordered;
  if (headsAll) {
    const first = ordered[0]!;
    rows.push({ kind: "span", label: labelFor(first), at: first.at });
  }
  let next = 0;
  for (const message of conversation) {
    while (next < inline.length && message.createdAt >= inline[next]!.at) {
      const span = inline[next]!;
      rows.push({ kind: "span", label: labelFor(span), at: span.at });
      next++;
    }
    rows.push({ kind: "history", message });
  }
  for (; next < inline.length; next++) {
    const span = inline[next]!;
    rows.push({ kind: "span", label: labelFor(span), at: span.at });
  }
};

/**
 * Model ranges (the same idea as node/agent spans): an assistant message's
 * `model` marks which model generated it, so a transition means the run
 * switched mid-session — delimit it with a marker before the first message
 * under the new model. The first model is the session default, not a
 * boundary, so it earns no marker.
 */
const insertModelMarkers = (rows: ChatRow[]): void => {
  let current: string | undefined;
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index]!;
    if (row.kind !== "history" || row.message.role !== "assistant") continue;
    const model = row.message.model;
    if (model === undefined) continue;
    if (current !== undefined && model !== current) {
      rows.splice(index, 0, {
        kind: "span",
        label: `model: ${model}`,
        at: row.message.createdAt,
      });
      index++;
    }
    current = model;
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
    // themselves surface as `tool` rows, so a blank bubble is pure noise. A
    // message whose only payload is an attachment is not blank, though.
    if (
      message.role === "assistant" &&
      message.content.trim() === "" &&
      !hasAttachments(message.blocks)
    ) {
      continue;
    }
    const prev = conversation[conversation.length - 1];
    if (
      prev !== undefined &&
      prev.role === message.role &&
      prev.content === message.content &&
      // Two identical texts that carry different attachments are not the
      // same turn — devin rewrites prompt text verbatim, attachments included.
      JSON.stringify(prev.blocks ?? null) === JSON.stringify(message.blocks ?? null)
    ) {
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
  if (spans !== undefined && spans.length > 0) {
    insertSpanMarkers(rows, conversation, spans, spanLabel);
  } else {
    for (const message of conversation) rows.push({ kind: "history", message });
  }
  // Runs after the span pass so a node/agent boundary + model change on the
  // same message stack both markers ahead of it (span first, then model).
  insertModelMarkers(rows);
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
  return live.every((m) => {
    if (m.role !== "user" && m.role !== "assistant") return true;
    const candidates = history.filter((h) => h.role === m.role && h.content === m.content);
    if (candidates.length === 0) return false;
    // A row sent with attachments only counts once the flush kept them —
    // stores that drop blocks would otherwise leave the live row ghosting.
    if (!hasAttachments(m.blocks)) return true;
    return candidates.some((h) => hasAttachments(h.blocks));
  });
};
