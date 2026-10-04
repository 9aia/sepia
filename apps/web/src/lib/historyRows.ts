import type { HistoryMessage } from "./types";
import type { LiveMessage } from "./liveMessages";
import type { SystemContext } from "./systemContext";

export type ChatRow =
  | { readonly kind: "system"; readonly context: SystemContext }
  | { readonly kind: "history"; readonly message: HistoryMessage }
  | { readonly kind: "live"; readonly message: LiveMessage };

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
): ChatRow[] => {
  const conversation: HistoryMessage[] = [];
  for (const message of history) {
    if (message.role === "system") continue;
    const prev = conversation[conversation.length - 1];
    if (prev !== undefined && prev.role === message.role && prev.content === message.content) {
      continue;
    }
    conversation.push(message);
  }
  const contextEmpty =
    context.workspaces.length === 0 &&
    context.rules.length === 0 &&
    context.promptText === "" &&
    context.platform === null;
  return [
    ...(contextEmpty ? [] : [{ kind: "system", context } satisfies ChatRow]),
    ...conversation.map((message): ChatRow => ({ kind: "history", message })),
    ...liveMessages.map((message): ChatRow => ({ kind: "live", message })),
  ];
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
