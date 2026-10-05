import { LockIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../../lib/types";

type LockState = Pick<SessionSummary, "locked" | "lockHolderPid">;

/**
 * Tooltip for a held row — names the holder pid when the probe reported one.
 * `null` while the session is free, which is also the marker's render gate.
 */
export const lockTooltip = (session: LockState): string | null => {
  if (session.locked !== true) return null;
  return session.lockHolderPid === null
    ? "Held by another process"
    : `Held by another process (PID ${session.lockHolderPid})`;
};

/**
 * Quiet held-state marker for a session row — a muted padlock with a tooltip.
 * Renders nothing while the session is free; attaching read-only is already
 * explained in the chat pane, so the row only needs the glanceable hint.
 */
export function LockMark({ session }: { readonly session: LockState }) {
  const title = lockTooltip(session);
  if (title === null) return null;
  return (
    <span
      className="inline-flex shrink-0 items-center text-muted-foreground"
      title={title}
      aria-label={title}
    >
      <HugeiconsIcon icon={LockIcon} strokeWidth={2} className="size-3.5" />
    </span>
  );
}
