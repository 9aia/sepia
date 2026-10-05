import type { TransferEndpoint } from "./api";
import { peerSecret, type PeerNode } from "./nodes";
import { localTarget } from "./targets";
import { getToken } from "./token";

/**
 * Project transfer endpoint resolution (docs/protocol.md "Project
 * transfer"). Pull/push are node-to-node — the client resolves a peer's
 * address + credential and hands them to the node doing the transfer, which
 * fetches/POSTs the bundle itself.
 */
const localOrigin = (): string =>
  localTarget().baseUrl !== ""
    ? localTarget().baseUrl
    : typeof location === "undefined"
      ? ""
      : location.origin;

/**
 * The `{url, token}` a node should use to reach `peer` during a transfer:
 *
 * - Direct peer → its registered origin + the client-held credential.
 * - `via: "gateway"` peer → this node's own `/api/gateway/<serverId>` mount,
 *   authenticated with the local token. The node loops back through its own
 *   gateway route, which injects the peer's stored credential — the only
 *   reachable path when the client can't touch the peer directly. Only
 *   correct when the transferring node IS this machine (pull-to-local,
 *   push-from-local); a peer→peer push through our gateway can't resolve —
 *   the peer can't reach our origin.
 */
export const peerEndpoint = (peer: PeerNode): TransferEndpoint => {
  if (peer.via === "gateway" && peer.serverId !== undefined) {
    return {
      url: `${localOrigin()}/api/gateway/${encodeURIComponent(peer.serverId)}`,
      token: getToken(),
    };
  }
  return { url: peer.url, token: peerSecret(peer) };
};

/**
 * The endpoint a peer should use to push BACK to this machine — the local
 * node's public address + the local token. Stands in for a peer row in the
 * push dialog's target list.
 */
export const localEndpoint = (): TransferEndpoint => ({
  url: localOrigin(),
  token: getToken(),
});
