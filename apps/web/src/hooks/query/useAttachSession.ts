import { useMutation } from "@tanstack/react-query";
import { attach } from "../../lib/api";

interface AttachInput {
  readonly id: string;
  readonly takeover?: boolean;
}

/**
 * Spawns/locks the ACP agent for a session. Also handles takeover — same
 * mutation, `takeover: true`. `variables` identifies which session the
 * latest result belongs to, guarding against stale selections.
 */
export const useAttachSession = () =>
  useMutation({ mutationFn: ({ id, takeover }: AttachInput) => attach(id, { takeover }) });
