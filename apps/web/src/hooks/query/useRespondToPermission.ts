import { useMutation } from "@tanstack/react-query";
import { respondToPermission } from "../../lib/api";

interface PermissionResponse {
  readonly sessionId: string;
  readonly requestId: string;
  readonly optionId: string | null;
}

export const useRespondToPermission = () =>
  useMutation({
    mutationFn: ({ sessionId, requestId, optionId }: PermissionResponse) =>
      respondToPermission(sessionId, requestId, optionId),
  });
