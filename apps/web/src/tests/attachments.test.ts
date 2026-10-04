import { describe, expect, it } from "vite-plus/test";
import {
  attachmentBudgetError,
  attachmentToPart,
  classifyFile,
  fileToAttachment,
  MAX_ATTACHMENTS,
  MAX_ATTACHMENT_BYTES,
  partToBlock,
  type PendingAttachment,
} from "../lib/attachments";

const attachment = (over: Partial<PendingAttachment> = {}): PendingAttachment => ({
  id: "a1",
  name: "file.bin",
  size: 4,
  mimeType: "application/octet-stream",
  kind: "binary",
  data: "AAE=",
  ...over,
});

describe("classifyFile", () => {
  it.each([
    ["shot.png", "image/png", "image"],
    ["icon.svg", "image/svg+xml", "image"],
    ["voice.mp3", "audio/mpeg", "audio"],
    ["notes.md", "text/markdown", "text"],
    ["data.json", "application/json", "text"],
    ["main.ts", "", "text"],
    ["archive.bin", "application/octet-stream", "binary"],
    ["movie.mp4", "video/mp4", "binary"],
  ] as const)("%s (%s) → %s", (name, mimeType, kind) => {
    expect(classifyFile(name, mimeType)).toBe(kind);
  });
});

describe("fileToAttachment", () => {
  it("reads text files as embedded text", async () => {
    const result = await fileToAttachment(
      new File(["# title"], "notes.md", {
        type: "text/markdown",
      }),
    );
    expect(result).toMatchObject({
      name: "notes.md",
      kind: "text",
      mimeType: "text/markdown",
      text: "# title",
    });
    expect(result.data).toBeUndefined();
  });

  it("reads images as base64 with a data-URL preview", async () => {
    const result = await fileToAttachment(
      new File([new Uint8Array([1, 2, 3])], "a.png", {
        type: "image/png",
      }),
    );
    expect(result).toMatchObject({ kind: "image", data: "AQID" });
    expect(result.previewUrl).toBe("data:image/png;base64,AQID");
  });

  it("reads other binaries as base64 without a preview", async () => {
    const result = await fileToAttachment(
      new File([new Uint8Array([0])], "a.bin", {
        type: "application/octet-stream",
      }),
    );
    expect(result).toMatchObject({ kind: "binary", data: "AA==" });
    expect(result.previewUrl).toBeUndefined();
  });
});

describe("attachmentToPart", () => {
  it("maps image/audio to base64 blocks", () => {
    expect(
      attachmentToPart(
        attachment({ kind: "image", name: "a.png", mimeType: "image/png", data: "aGk=" }),
      ),
    ).toEqual({
      type: "image",
      data: "aGk=",
      mimeType: "image/png",
      uri: "attachment://a.png",
    });
    expect(
      attachmentToPart(attachment({ kind: "audio", mimeType: "audio/mpeg", data: "AAA=" })),
    ).toEqual({ type: "audio", data: "AAA=", mimeType: "audio/mpeg" });
  });

  it("maps text/binary files to embedded resources carrying the name in the uri", () => {
    expect(
      attachmentToPart(
        attachment({ kind: "text", name: "n.md", mimeType: "text/markdown", text: "hi" }),
      ),
    ).toEqual({
      type: "resource",
      resource: { uri: "attachment://n.md", mimeType: "text/markdown", text: "hi" },
    });
    expect(attachmentToPart(attachment({ kind: "binary", name: "b.bin", data: "AAE=" }))).toEqual({
      type: "resource",
      resource: {
        uri: "attachment://b.bin",
        mimeType: "application/octet-stream",
        blob: "AAE=",
      },
    });
  });
});

describe("partToBlock", () => {
  it("round-trips the parts attachmentToPart produces into IR blocks", () => {
    const image = attachment({ kind: "image", name: "a.png", mimeType: "image/png" });
    expect(partToBlock(attachmentToPart(image))).toEqual({
      type: "image",
      data: "AAE=",
      mimeType: "image/png",
      uri: "attachment://a.png",
    });

    const text = attachment({
      kind: "text",
      name: "n.md",
      mimeType: "text/markdown",
      text: "hi",
      data: undefined,
    });
    expect(partToBlock(attachmentToPart(text))).toEqual({
      type: "file",
      uri: "attachment://n.md",
      mimeType: "text/markdown",
      text: "hi",
    });

    expect(partToBlock(attachmentToPart(attachment({ name: "b.bin" })))).toEqual({
      type: "file",
      uri: "attachment://b.bin",
      mimeType: "application/octet-stream",
      data: "AAE=",
    });
  });

  it("maps resource_link to a named file block", () => {
    expect(partToBlock({ type: "resource_link", uri: "file:///x", name: "x", size: 3 })).toEqual({
      type: "file",
      uri: "file:///x",
      name: "x",
      size: 3,
    });
  });
});

describe("attachmentBudgetError", () => {
  it("accepts a batch under the caps", () => {
    expect(attachmentBudgetError([], [{ size: 1024 }])).toBeNull();
  });

  it("rejects when the count would exceed the cap", () => {
    const existing = Array.from({ length: MAX_ATTACHMENTS }, (_, i) => attachment({ id: `a${i}` }));
    expect(attachmentBudgetError(existing, [{ size: 1 }])).toContain("Too many attachments");
    expect(
      attachmentBudgetError([], [{ size: 1 }], { count: MAX_ATTACHMENTS, bytes: 0 }),
    ).toContain("Too many attachments");
  });

  it("rejects when total bytes would exceed the cap, counting reserved reads", () => {
    expect(
      attachmentBudgetError([attachment({ size: MAX_ATTACHMENT_BYTES - 10 })], [{ size: 11 }]),
    ).toContain("over the 5MB limit");
    expect(
      attachmentBudgetError([], [{ size: 10 }], {
        count: 1,
        bytes: MAX_ATTACHMENT_BYTES,
      }),
    ).toContain("over the 5MB limit");
  });
});
