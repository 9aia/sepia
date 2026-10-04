export interface ParsedError {
  readonly code?: string;
  readonly message: string;
}

/** "rate_limit_exceeded" → "Rate Limit Exceeded". */
export const prettifyCode = (code: string): string =>
  code
    .toLowerCase()
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");

const hasErrorMarker = (record: Record<string, unknown>): boolean =>
  typeof record.code === "string" ||
  record.type === "error" ||
  record.status === "error" ||
  record.isError === true;

/**
 * Detects JSON error payloads agents emit as message content. Requires an
 * explicit error signal — an `error` member (object or string), or a
 * `message` paired with `code`/`type:"error"`/`status:"error"`/`isError` —
 * so an assistant answer that simply quotes a `{ "message": … }` shape is
 * not swallowed into an error card.
 */
export const parseErrorPayload = (content: string): ParsedError | null => {
  const trimmed = content.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
    const record = parsed as Record<string, unknown>;

    const error = record.error;
    if (typeof error === "string" && error !== "") {
      return {
        ...(typeof record.code === "string" ? { code: record.code } : {}),
        message: error,
      };
    }
    const inner =
      typeof error === "object" && error !== null
        ? (error as Record<string, unknown>)
        : hasErrorMarker(record)
          ? record
          : null;
    if (inner === null || typeof inner.message !== "string") return null;
    return {
      ...(typeof inner.code === "string"
        ? { code: inner.code }
        : typeof record.code === "string"
          ? { code: record.code }
          : {}),
      message: inner.message,
    };
  } catch {
    return null;
  }
};
