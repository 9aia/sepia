import type { HistoryBlock } from "./types";

/**
 * The wire shape of one prompt content block — the ACP `session/prompt`
 * `ContentBlock` subset the server's `POST /api/sessions/:id/prompt` accepts
 * (mirrors `PromptPart` in sepia-acp; re-declared here so the web bundle
 * doesn't depend on the node-side package).
 */
export type PromptPart =
  | { readonly type: "text"; readonly text: string }
  | {
      readonly type: "image";
      readonly data: string;
      readonly mimeType: string;
      readonly uri?: string;
    }
  | { readonly type: "audio"; readonly data: string; readonly mimeType: string }
  | {
      readonly type: "resource";
      readonly resource:
        | { readonly uri: string; readonly mimeType?: string; readonly text: string }
        | { readonly uri: string; readonly mimeType?: string; readonly blob: string };
    }
  | {
      readonly type: "resource_link";
      readonly uri: string;
      readonly name: string;
      readonly mimeType?: string;
      readonly size?: number;
    };

export type AttachmentKind = "image" | "audio" | "text" | "binary";

/** A composer attachment — the file's payload is read eagerly so send is synchronous. */
export interface PendingAttachment {
  readonly id: string;
  readonly name: string;
  /** Byte size, shown on the chip and counted against the budget. */
  readonly size: number;
  readonly mimeType: string;
  readonly kind: AttachmentKind;
  /** base64 payload for image/audio/binary kinds. */
  readonly data?: string;
  /** UTF-8 payload for the text kind. */
  readonly text?: string;
  /** `data:` URL — set for images so the chip shows a thumbnail. */
  readonly previewUrl?: string;
}

export const MAX_ATTACHMENTS = 8;
/** Total raw bytes across one send (~6.7MB of base64 on the wire). */
export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;

const TEXT_MIME =
  /^(text\/|application\/(json|xml|javascript|x-javascript|x-sh|x-yaml|yaml|toml|x-toml|sql|x-httpd-php|x-perl|x-python))/;

const TEXT_EXT =
  /\.(txt|md|markdown|log|csv|tsv|json|ya?ml|toml|xml|html?|css|jsx?|tsx?|mjs|cjs|py|pyi|rb|go|rs|java|kt|c|cc|cpp|h|hpp|sh|bash|zsh|fish|sql|env|ini|cfg|conf|diff|patch|gitignore|dockerfile)$/i;

/**
 * What the file turns into on the wire. `image/` is checked first —
 * `image/svg+xml` matches the text pattern too, but it renders as a
 * thumbnail so it belongs on the image path.
 */
export const classifyFile = (name: string, mimeType: string): AttachmentKind => {
  if (mimeType.startsWith("image/")) return "image";
  if (mimeType.startsWith("audio/")) return "audio";
  if (TEXT_MIME.test(mimeType) || TEXT_EXT.test(name)) return "text";
  return "binary";
};

/**
 * The URI embedded resources carry — `attachment://` marks it as
 * client-supplied (not resolvable), and the name survives inside it so the
 * flushed IR's `file` block still renders a named chip.
 */
const attachmentUri = (name: string): string => `attachment://${name}`;

/** Pending attachment → the ACP content block sent with the prompt. */
export const attachmentToPart = (attachment: PendingAttachment): PromptPart => {
  switch (attachment.kind) {
    case "image":
      return {
        type: "image",
        data: attachment.data ?? "",
        mimeType: attachment.mimeType,
        uri: attachmentUri(attachment.name),
      };
    case "audio":
      return { type: "audio", data: attachment.data ?? "", mimeType: attachment.mimeType };
    case "text":
      return {
        type: "resource",
        resource: {
          uri: attachmentUri(attachment.name),
          mimeType: attachment.mimeType,
          text: attachment.text ?? "",
        },
      };
    case "binary":
      return {
        type: "resource",
        resource: {
          uri: attachmentUri(attachment.name),
          mimeType: attachment.mimeType,
          blob: attachment.data ?? "",
        },
      };
  }
};

/**
 * The same part as an IR `Block` — what the optimistic row and the flushed
 * history render through `attachmentViews` (`resource`/`resource_link` both
 * land on `file`, matching `blockFromAcp` in sepia-core).
 */
export const partToBlock = (part: PromptPart): HistoryBlock => {
  switch (part.type) {
    case "text":
      return { type: "text", text: part.text };
    case "image":
      return {
        type: "image",
        data: part.data,
        mimeType: part.mimeType,
        ...(part.uri !== undefined ? { uri: part.uri } : {}),
      };
    case "audio":
      return { type: "audio", data: part.data, mimeType: part.mimeType };
    case "resource":
      return {
        type: "file",
        uri: part.resource.uri,
        ...(part.resource.mimeType !== undefined ? { mimeType: part.resource.mimeType } : {}),
        ...("text" in part.resource ? { text: part.resource.text } : { data: part.resource.blob }),
      };
    case "resource_link":
      return {
        type: "file",
        uri: part.uri,
        name: part.name,
        ...(part.mimeType !== undefined ? { mimeType: part.mimeType } : {}),
        ...(part.size !== undefined ? { size: part.size } : {}),
      };
  }
};

const toBase64 = (buffer: ArrayBuffer): string => {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
};

/** Reads the file eagerly — chips and send never touch it again. */
export const fileToAttachment = async (
  file: File,
  id: string = crypto.randomUUID(),
): Promise<PendingAttachment> => {
  const mimeType = file.type === "" ? "application/octet-stream" : file.type;
  const name = file.name === "" ? "attachment" : file.name;
  const kind = classifyFile(name, mimeType);
  if (kind === "text") {
    return { id, name, size: file.size, mimeType, kind, text: await file.text() };
  }
  const data = toBase64(await file.arrayBuffer());
  return {
    id,
    name,
    size: file.size,
    mimeType,
    kind,
    data,
    ...(kind === "image" ? { previewUrl: `data:${mimeType};base64,${data}` } : {}),
  };
};

export const formatAttachmentSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
};

/**
 * The budget check for a batch of incoming files — returns the error to
 * surface, or null when the batch fits (count + total bytes). `reserved`
 * covers files still being read into attachments, so back-to-back drops
 * can't race past the cap.
 */
export const attachmentBudgetError = (
  existing: ReadonlyArray<PendingAttachment>,
  incoming: ReadonlyArray<{ readonly size: number }>,
  reserved: { readonly count: number; readonly bytes: number } = { count: 0, bytes: 0 },
): string | null => {
  if (existing.length + reserved.count + incoming.length > MAX_ATTACHMENTS) {
    return `Too many attachments — ${MAX_ATTACHMENTS} max`;
  }
  const total =
    existing.reduce((sum, a) => sum + a.size, reserved.bytes) +
    incoming.reduce((sum, f) => sum + f.size, 0);
  if (total > MAX_ATTACHMENT_BYTES) {
    return `Attachments are over the ${Math.round(MAX_ATTACHMENT_BYTES / (1024 * 1024))}MB limit`;
  }
  return null;
};
