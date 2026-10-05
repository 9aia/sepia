import { settingsStore } from "./settings";
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

/**
 * The target for "the local node" — the machine this client treats as its
 * own. Token read lazily.
 *
 * Default (`settings.localNodeUrl === null`): `baseUrl: ""` — relative,
 * same-origin requests. That's the transport the vite dev proxy relies on
 * (UI on :3000 forwarding /api to :8787), so the default must stay relative.
 *
 * With `localNodeUrl` set, EVERY local call — fan-out legs, session actions,
 * the events feed, `/api/servers` and `/api/gateway/*` hops — goes to that
 * absolute origin instead of the UI host. That's the seam for pointing the
 * client at a different node than the one serving the page; the serving
 * origin is only ever the default, not an identity.
 *
 * The token is bound to the address it was entered for (lib/token.ts): a
 * repointed override yields `token: null` — the node's own credential must
 * be re-entered, and this origin's token is never sent to a different host.
 */
export const localTarget = (): ApiTarget => ({
  baseUrl: settingsStore.state.localNodeUrl ?? "",
  token: getToken(),
});
