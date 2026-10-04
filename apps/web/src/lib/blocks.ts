import type { HistoryBlock } from "./types";

/**
 * A renderable attachment: `image` inlines as a thumbnail, `file` (and
 * non-renderable images/audio) collapse to a chip with a name and detail.
 */
export type AttachmentView =
  | { readonly kind: "image"; readonly src: string; readonly alt: string }
  | { readonly kind: "file"; readonly name: string; readonly detail: string };

/** True when a block list carries something `content` cannot show. */
export const hasAttachments = (blocks: ReadonlyArray<HistoryBlock> | undefined): boolean =>
  blocks !== undefined && blocks.some((block) => block.type !== "text");

/** Last path segment of a uri, scheme-stripped — the chip label fallback. */
const uriName = (uri: string): string => {
  const trimmed = uri.replace(/^file:\/\//, "").replace(/\/+$/, "");
  const last = trimmed.split("/").pop();
  return last === undefined || last === "" ? uri : last;
};

const formatSize = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
};

/**
 * Non-text blocks → views `MessageRow` renders under the message text.
 * Text blocks are skipped — `content` already projects them — and an image
 * with neither inline data nor a uri degrades to a chip rather than a
 * broken `<img>`.
 */
export const attachmentViews = (
  blocks: ReadonlyArray<HistoryBlock> | undefined,
): AttachmentView[] => {
  if (blocks === undefined) return [];
  const views: AttachmentView[] = [];
  for (const block of blocks) {
    switch (block.type) {
      case "image": {
        const src =
          block.data !== undefined
            ? `data:${block.mimeType ?? "image/png"};base64,${block.data}`
            : block.uri;
        if (src !== undefined) {
          views.push({
            kind: "image",
            src,
            alt: block.uri !== undefined ? uriName(block.uri) : (block.mimeType ?? "image"),
          });
        } else {
          views.push({ kind: "file", name: "image", detail: block.mimeType ?? "" });
        }
        break;
      }
      case "file":
      case "audio": {
        const name =
          ("name" in block ? block.name : undefined) ??
          ("uri" in block && block.uri !== undefined ? uriName(block.uri) : undefined) ??
          block.type;
        const detail = [
          block.mimeType ?? "",
          "size" in block && block.size !== undefined ? formatSize(block.size) : "",
        ]
          .filter((part) => part !== "")
          .join(" · ");
        views.push({ kind: "file", name, detail });
        break;
      }
      default:
        break;
    }
  }
  return views;
};
