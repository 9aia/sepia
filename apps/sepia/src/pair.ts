import { getRandomValues } from "node:crypto";
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/**
 * The mint half of pairing (docs/protocol.md "Auth — pairing"): `sepia pair`
 * runs on the node itself and writes `$SEPIA_HOME/pair-code` —
 * `{code, expiresAt}` — which the running server consumes on the next
 * `POST /api/pair`. The filesystem is the gate: only someone who can write
 * to $SEPIA_HOME (the machine owner) can mint a code. The file format and
 * code alphabet mirror `apps/server/src/pair.ts` — keep them in sync.
 */

export const PAIR_CODE_TTL_MS = 60_000;

// Crockford base32 — no I/L/O/U, so codes survive being read aloud or typed.
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** 8 Crockford chars in a 4-4 group, e.g. `7K2M-9PQX` (~40 bits). */
export const mintPairCode = (): string => {
  const bytes = getRandomValues(new Uint8Array(8));
  const chars = Array.from(bytes, (byte) => CROCKFORD[byte % CROCKFORD.length]);
  return `${chars.slice(0, 4).join("")}-${chars.slice(4).join("")}`;
};

export interface MintedPairCode {
  readonly code: string;
  readonly expiresAt: number;
  /** Where the code was written — for messages, never logged verbatim. */
  readonly path: string;
}

/**
 * Atomically write the code file (0600, tmp+rename) so the server never
 * reads a half-written or world-readable code.
 */
export const writePairCodeFile = (home: string, now: number = Date.now()): MintedPairCode => {
  const code = mintPairCode();
  const expiresAt = now + PAIR_CODE_TTL_MS;
  mkdirSync(home, { recursive: true });
  const path = join(home, "pair-code");
  const tmp = `${path}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify({ code, expiresAt }), { mode: 0o600 });
  renameSync(tmp, path);
  return { code, expiresAt, path };
};
