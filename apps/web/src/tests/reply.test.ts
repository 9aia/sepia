import { describe, expect, it } from "vite-plus/test";
import { formatReplyPrompt, replyAuthorLabel, type ReplyQuote } from "../lib/reply";
import { sepiaStore, setReplyTo, setSelectedId } from "../lib/store";

const quote = (content: string, role: ReplyQuote["role"] = "assistant"): ReplyQuote => ({
  role,
  content,
});

describe("replyAuthorLabel", () => {
  it("maps roles to sender labels", () => {
    expect(replyAuthorLabel("user")).toBe("You");
    expect(replyAuthorLabel("assistant")).toBe("Assistant");
  });
});

describe("formatReplyPrompt", () => {
  it("blockquotes each line and appends the reply text", () => {
    expect(formatReplyPrompt(quote("line one\nline two"), "my reply")).toBe(
      "> line one\n> line two\n>\n> — Assistant\n\nmy reply",
    );
  });

  it("labels user-authored quotes as You", () => {
    expect(formatReplyPrompt(quote("hi", "user"), "re: hi")).toContain("> — You");
  });

  it("keeps blank lines inside the quote as bare >", () => {
    expect(formatReplyPrompt(quote("a\n\nb"), "x")).toBe("> a\n>\n> b\n>\n> — Assistant\n\nx");
  });

  it("trims a trailing newline instead of emitting a trailing quote line", () => {
    expect(formatReplyPrompt(quote("ends with newline\n"), "x")).toBe(
      "> ends with newline\n>\n> — Assistant\n\nx",
    );
  });

  it("handles empty quoted content", () => {
    expect(formatReplyPrompt(quote(""), "still replying")).toBe(
      ">\n>\n> — Assistant\n\nstill replying",
    );
  });

  it("handles long content without truncating", () => {
    const long = "word ".repeat(500).trim();
    const out = formatReplyPrompt(quote(long), "x");
    expect(out.startsWith(`> ${long}\n>\n> — Assistant\n\nx`)).toBe(true);
  });

  it("returns just the quote block when the reply text is empty", () => {
    expect(formatReplyPrompt(quote("hi"), "   ")).toBe("> hi\n>\n> — Assistant");
  });
});

describe("replyTo store", () => {
  it("sets and clears the pending reply", () => {
    setReplyTo(quote("hello"));
    expect(sepiaStore.state.replyTo?.content).toBe("hello");
    setReplyTo(null);
    expect(sepiaStore.state.replyTo).toBeNull();
  });

  it("clears the reply when the selected session changes", () => {
    setSelectedId("session-a");
    setReplyTo(quote("hello"));
    setSelectedId("session-b");
    expect(sepiaStore.state.replyTo).toBeNull();
  });

  it("keeps the reply when setSelectedId is called with the same id", () => {
    setSelectedId("session-c");
    setReplyTo(quote("hello"));
    setSelectedId("session-c");
    expect(sepiaStore.state.replyTo?.content).toBe("hello");
    setReplyTo(null);
  });
});
