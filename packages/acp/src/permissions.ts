import { normalizePermission } from "./normalize.js";
import type { PermissionRequest } from "./types.js";

/** ACP `session/request_permission` response, shaped for the SDK's handler return. */
export type PermissionOutcome =
  | { readonly outcome: { readonly outcome: "selected"; readonly optionId: string } }
  | { readonly outcome: { readonly outcome: "cancelled" } };

export interface PendingPermission {
  readonly request: PermissionRequest;
  readonly response: Promise<PermissionOutcome>;
}

export interface PermissionBroker {
  begin(params: unknown): PendingPermission;
  respond(requestId: string, optionId: string | null): boolean;
  failAll(error: Error): void;
}

interface Pending {
  readonly resolve: (outcome: PermissionOutcome) => void;
  readonly reject: (error: Error) => void;
}

/**
 * Holds permission requests until a human answers. The ACP handler returns the
 * pending `response`; `respond` settles it out of band.
 */
export const createPermissionBroker = (): PermissionBroker => {
  const pending = new Map<string, Pending>();

  const settle = (requestId: string, optionId: string | null): boolean => {
    const entry = pending.get(requestId);
    if (entry === undefined) return false;
    pending.delete(requestId);
    entry.resolve(
      optionId === null
        ? { outcome: { outcome: "cancelled" } }
        : { outcome: { outcome: "selected", optionId } },
    );
    return true;
  };

  return {
    begin: (params) => {
      const request = normalizePermission(params);
      const response = new Promise<PermissionOutcome>((resolve, reject) =>
        pending.set(request.requestId, { resolve, reject }),
      );
      return { request, response };
    },
    respond: (requestId, optionId) => settle(requestId, optionId),
    failAll: (error) => {
      const entries = [...pending.values()];
      pending.clear();
      for (const entry of entries) entry.reject(error);
    },
  };
};
