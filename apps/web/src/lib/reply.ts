/** The message a pending prompt is replying to (quoted in the composer). */
export interface ReplyQuote {
  readonly id?: string;
  readonly role: "user" | "assistant";
  readonly content: string;
  readonly createdAt?: number;
}

/** Sender label shown in the composer preview and embedded in the prompt. */
export const replyAuthorLabel = (role: ReplyQuote["role"]): string =>
  role === "user" ? "You" : "Assistant";

/**
 * Prompt text for a reply: the quoted message as a markdown blockquote —
 * one "> "-prefixed line per source line, then an attribution line —
 * followed by the user's own text.
 */
export const formatReplyPrompt = (quote: ReplyQuote, text: string): string => {
  const quoted = quote.content
    .trimEnd()
    .split("\n")
    .map((line) => (line === "" ? ">" : `> ${line}`))
    .join("\n");
  const block = `${quoted}\n>\n> — ${replyAuthorLabel(quote.role)}`;
  const trimmed = text.trim();
  return trimmed === "" ? block : `${block}\n\n${trimmed}`;
};
