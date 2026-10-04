import { getToken } from "./token";

/**
 * Where an API call lands. `baseUrl` is "" for the same-origin node (the
 * local machine serving the UI) and a full origin like
 * `https://thinkpad:8787` for a federated peer. `timeoutMs` bounds peer
 * fan-out so a dead node can't stall merged lists (docs/protocol.md).
 */
export interface ApiTarget {
  readonly baseUrl: string;
  readonly token: string | null;
  readonly timeoutMs?: number;
}

/** Same-origin target — the node serving this UI. Token read lazily. */
export const localTarget = (): ApiTarget => ({ baseUrl: "", token: getToken() });
