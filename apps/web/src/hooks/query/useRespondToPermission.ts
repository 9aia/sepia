import { useMutation } from "@tanstack/react-query";
import { respondToPermission } from "../../lib/api";
import { nodeTarget } from "../../lib/nodes";

interface PermissionResponse {
  readonly sessionId: string;
  readonly agent?: string;
  readonly node?: string;
  readonly requestId: string;
  readonly optionId: string | null;
}

export const useRespondToPermission = () =>
  useMutation({
    mutationFn: ({ sessionId, agent, node, requestId, optionId }: PermissionResponse) =>
      respondToPermission(sessionId, requestId, optionId, agent, nodeTarget(node)),
  });
