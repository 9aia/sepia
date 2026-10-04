import { useMutation, useQuery } from "@tanstack/react-query";
import { getCheckpoints, restoreSession, type RestoreSelector } from "../../lib/api";
import { nodeTarget } from "../../lib/nodes";
import type { RestoreResult } from "../../lib/types";
import { queryKeys } from "./keys";

/**
 * The workspace snapshot refs a session recorded (Cline shadow-git
 * checkpoints) — fetched lazily by the details drawer's restore dialog.
 * `enabled` gates the fetch so closed sessions never hit the endpoint.
 */
export const useCheckpoints = (
  sessionId: string | null,
  agent?: string,
  node?: string,
  enabled = true,
) =>
  useQuery({
    queryKey: queryKeys.checkpoints(sessionId ?? "", agent, node),
    queryFn: () => getCheckpoints(sessionId as string, agent, nodeTarget(node)),
    enabled: enabled && sessionId !== null && sessionId !== "",
  });

/** One-line summary a restore result toast shows. */
export const restoreSummary = (result: RestoreResult): string => {
  const changed = result.restored.filter((r) => r.action !== "unchanged");
  const parts: string[] = [];
  if (changed.length > 0)
    parts.push(`${changed.length} file${changed.length === 1 ? "" : "s"} restored`);
  if (result.skipped.length > 0) parts.push(`${result.skipped.length} skipped`);
  return parts.length === 0 ? "Nothing to restore" : parts.join(" · ");
};

/**
 * Restore files under a session's working directory — a per-call diff revert
 * (`{path, toolCallId?}`) or a checkpoint materialization (`{checkpoint}`).
 * Callers toast on success/error; the session must not be busy or held.
 */
export const useRestoreSession = () =>
  useMutation({
    mutationFn: ({
      sessionId,
      agent,
      node,
      selector,
    }: {
      sessionId: string;
      agent?: string;
      node?: string;
      selector: RestoreSelector;
    }): Promise<RestoreResult> => restoreSession(sessionId, selector, agent, nodeTarget(node)),
  });
