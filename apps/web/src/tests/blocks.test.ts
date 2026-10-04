import { describe, expect, it } from "vite-plus/test";
import { attachmentViews, hasAttachments } from "../lib/blocks";
import { buildRows } from "../lib/historyRows";
import type { HistoryMessage } from "../lib/types";

const emptyContext = {
  workspaces: [],
  rules: [],
  reports: [],
  promptText: "",
  platform: null,
  osVersion: null,
  date: null,
};

describe("hasAttachments", () => {
  it("is true only when a non-text block is present", () => {
    expect(hasAttachments(undefined)).toBe(false);
    expect(hasAttachments([])).toBe(false);
    expect(hasAttachments([{ type: "text", text: "hi" }])).toBe(false);
    expect(
      hasAttachments([
        { type: "text", text: "hi" },
        { type: "image", uri: "x" },
      ]),
    ).toBe(true);
  });
});

describe("attachmentViews", () => {
  it("maps embedded and linked images to inline views", () => {
    expect(
      attachmentViews([
        { type: "text", text: "look" },
        { type: "image", data: "aGk=", mimeType: "image/png" },
        { type: "image", uri: "file:///shots/one.png" },
        { type: "image", data: "AAE=" },
      ]),
    ).toEqual([
      { kind: "image", src: "data:image/png;base64,aGk=", alt: "image/png" },
      { kind: "image", src: "file:///shots/one.png", alt: "one.png" },
      { kind: "image", src: "data:image/png;base64,AAE=", alt: "image" },
    ]);
  });

  it("maps files and audio to chips with a name and detail", () => {
    expect(
      attachmentViews([
        { type: "file", uri: "file:///work/spec.md", mimeType: "text/markdown", size: 2048 },
        { type: "file", name: "notes.txt", size: 3 },
        { type: "audio", data: "AAE=", mimeType: "audio/wav" },
        { type: "file", uri: "file:///" },
      ]),
    ).toEqual([
      { kind: "file", name: "spec.md", detail: "text/markdown · 2.0 KB" },
      { kind: "file", name: "notes.txt", detail: "3 B" },
      { kind: "file", name: "audio", detail: "audio/wav" },
      { kind: "file", name: "file:///", detail: "" },
    ]);
  });

  it("degrades an image with neither data nor uri to a chip", () => {
    expect(attachmentViews([{ type: "image", mimeType: "image/png" }])).toEqual([
      { kind: "file", name: "image", detail: "image/png" },
    ]);
  });

  it("returns nothing for absent or text-only lists", () => {
    expect(attachmentViews(undefined)).toEqual([]);
    expect(attachmentViews([{ type: "text", text: "x" }])).toEqual([]);
  });
});

describe("buildRows with blocks", () => {
  const msg = (over: Partial<HistoryMessage>): HistoryMessage => ({
    role: "assistant",
    content: "",
    createdAt: 1,
    ...over,
  });

  it("keeps a content-less row whose payload is an attachment", () => {
    const rows = buildRows(
      [msg({ blocks: [{ type: "file", name: "a.ts" }] }), msg({ content: "text" })],
      [],
      emptyContext,
    );
    expect(rows).toHaveLength(2);
  });

  it("still drops empty assistant rows without attachments", () => {
    const rows = buildRows([msg({}), msg({ content: "text" })], [], emptyContext);
    expect(rows).toHaveLength(1);
  });

  it("dedups adjacent same-text rows only when their blocks match too", () => {
    const a = msg({ role: "user", content: "go", blocks: [{ type: "image", uri: "a.png" }] });
    const b = msg({ role: "user", content: "go", blocks: [{ type: "image", uri: "b.png" }] });
    const aAgain = msg({ role: "user", content: "go", blocks: [{ type: "image", uri: "a.png" }] });
    // a and b are the same text but different attachments → both survive.
    expect(buildRows([a, b], [], emptyContext)).toHaveLength(2);
    // a repeated verbatim (devin rewrites prompts) → collapses as before.
    expect(buildRows([a, aAgain], [], emptyContext)).toHaveLength(1);
  });
});
