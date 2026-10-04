import { useMutation } from "@tanstack/react-query";
import { respondToPermission } from "../../lib/api";

interface PermissionResponse {
  readonly sessionId: string;
  readonly agent?: string;
  readonly requestId: string;
  readonly optionId: string | null;
}

export const useRespondToPermission = () =>
  useMutation({
    mutationFn: ({ sessionId, agent, requestId, optionId }: PermissionResponse) =>
      respondToPermission(sessionId, requestId, optionId, agent),
  });
