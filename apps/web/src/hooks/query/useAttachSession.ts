import { useMutation } from "@tanstack/react-query";
import { attach } from "../../lib/api";

interface AttachInput {
  readonly id: string;
  readonly agent?: string;
  readonly takeover?: boolean;
  readonly model?: string;
  readonly fallbacks?: ReadonlyArray<string>;
}

/**
 * Spawns/locks the ACP agent for a session. Also handles takeover — same
 * mutation, `takeover: true`. `variables` identifies which session the
 * latest result belongs to, guarding against stale selections.
 */
export const useAttachSession = () =>
  useMutation({
    mutationFn: ({ id, agent, takeover, model, fallbacks }: AttachInput) =>
      attach(id, { takeover, model, fallbacks, agent }),
  });
