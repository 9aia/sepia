import { createHash, getRandomValues, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/**
 * Short-code → long-lived credential exchange (docs/protocol.md "Auth —
 * pairing"). `sepia pair` mints a one-time code on the node by writing
 * `$SEPIA_HOME/pair-code` — the filesystem is the mint gate, so only the
 * machine owner (who can write to $SEPIA_HOME) can mint. The server absorbs
 * that file into an in-memory `PairingStore` on the next `POST /api/pair`
 * and redeems it for a `sepia_…` bearer credential.
 *
 * Codes are single-use and live ~60s; issued credentials are persisted as
 * sha256 hashes in `$SEPIA_HOME/tokens.json` so they survive restarts while
 * the file itself is never usable as a credential. Codes and tokens are
 * never logged (the access log records only the request path).
 */

/** How long a minted code stays redeemable (docs/protocol.md: ~60s). */
export const PAIR_CODE_TTL_MS = 60_000;

// Crockford base32 — no I/L/O/U, so codes survive being read aloud or typed.
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** 8 Crockford chars in a 4-4 group, e.g. `7K2M-9PQX` (~40 bits). */
export const mintPairCode = (): string => {
  const bytes = getRandomValues(new Uint8Array(8));
  const chars = Array.from(bytes, (byte) => CROCKFORD[byte % CROCKFORD.length]);
  return `${chars.slice(0, 4).join("")}-${chars.slice(4).join("")}`;
};

/** User input → lookup key: case-insensitive, dashes/spaces optional. */
export const normalizePairCode = (input: string): string =>
  input.toUpperCase().replace(/[^0-9A-Z]/g, "");

/** The long-lived credential handed out on redeem. */
export const mintPairToken = (): string => `sepia_${randomBytes(32).toString("base64url")}`;

/** Tokens persist as hashes only — the file alone can't authenticate. */
export const hashPairToken = (token: string): string =>
  createHash("sha256").update(token).digest("hex");

/**
 * The on-disk mint channel (`sepia pair` → server). Read on every
 * `POST /api/pair` and deleted on first read — single-use at the file
 * level; the in-memory TTL still governs retries within the code's life.
 */
interface PairCodeFile {
  readonly code: string;
  readonly expiresAt: number;
}

const readPairCodeFile = (path: string): PairCodeFile | null => {
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as Record<string, unknown>;
    if (
      typeof record.code !== "string" ||
      typeof record.expiresAt !== "number" ||
      !Number.isFinite(record.expiresAt)
    ) {
      return null;
    }
    return { code: record.code, expiresAt: record.expiresAt };
  } catch {
    return null;
  }
};

/**
 * In-memory one-time codes. `mint()` covers in-process mint paths (tests,
 * future loopback mint); `register()` absorbs codes minted out-of-process
 * by `sepia pair`. Expired entries are swept on every access so the map
 * can't grow via abandoned codes.
 */
export class PairingStore {
  private readonly codes = new Map<string, number>();

  constructor(private readonly ttlMs: number = PAIR_CODE_TTL_MS) {}

  private sweep(now: number): void {
    for (const [code, expiresAt] of this.codes) {
      if (expiresAt <= now) this.codes.delete(code);
    }
  }

  /** Mint a fresh code; returns what's safe to print to the node operator. */
  mint(now: number = Date.now()): { readonly code: string; readonly expiresAt: number } {
    const code = mintPairCode();
    const expiresAt = now + this.ttlMs;
    this.register(code, expiresAt);
    return { code, expiresAt };
  }

  /** Register an externally minted code (the $SEPIA_HOME/pair-code file). */
  register(code: string, expiresAt: number): void {
    const normalized = normalizePairCode(code);
    if (normalized === "") return;
    this.codes.set(normalized, expiresAt);
  }

  /**
   * Single-use redeem: consumes the code on success; an unknown, malformed
   * or expired code just returns false (callers map that to one 404 — never
   * distinguish expired vs never-existed).
   */
  redeem(code: string, now: number = Date.now()): boolean {
    this.sweep(now);
    const key = normalizePairCode(code);
    if (!this.codes.has(key)) return false;
    this.codes.delete(key);
    return true;
  }
}

/** The seam `createApp` consumes: redeem a code, check a credential. */
export interface Pairing {
  /**
   * `POST /api/pair` — absorbs any pending code file, then redeems. On
   * success returns a fresh long-lived credential (already persisted).
   */
  readonly redeem: (code: string) => string | null;
  /** Bearer-auth check for credentials this node has issued. */
  readonly accepts: (token: string) => boolean;
}

export interface PairingOptions {
  /** `$SEPIA_HOME/pair-code` — written by `sepia pair`, consumed on read. */
  readonly codeFile: string;
  /** `$SEPIA_HOME/tokens.json` — sha256 hashes of issued credentials. */
  readonly tokensFile: string;
  readonly store?: PairingStore;
  /** Test seam for expiry; defaults to wall clock. */
  readonly now?: () => number;
}

export const createPairing = (options: PairingOptions): Pairing => {
  const store = options.store ?? new PairingStore();
  const now = options.now ?? (() => Date.now());
  const hashes = new Set<string>();

  if (existsSync(options.tokensFile)) {
    try {
      const parsed = JSON.parse(readFileSync(options.tokensFile, "utf8")) as unknown;
      if (typeof parsed === "object" && parsed !== null) {
        const tokens = (parsed as Record<string, unknown>).tokens;
        if (Array.isArray(tokens)) {
          for (const token of tokens) {
            if (typeof token === "string") hashes.add(token);
          }
        }
      }
    } catch {
      // A corrupt credential file degrades to empty — operators re-pair.
    }
  }

  const flushTokens = (): void => {
    mkdirSync(dirname(options.tokensFile), { recursive: true });
    const tmp = `${options.tokensFile}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify({ tokens: [...hashes] }), { mode: 0o600 });
    renameSync(tmp, options.tokensFile);
  };

  // The mint gate: whoever could write $SEPIA_HOME/pair-code is the machine
  // owner. Consume the file regardless of contents so a stale code can't
  // linger past its TTL or be replayed after a successful redeem.
  const absorbCodeFile = (): void => {
    if (!existsSync(options.codeFile)) return;
    const file = readPairCodeFile(options.codeFile);
    try {
      rmSync(options.codeFile, { force: true });
    } catch {
      // A file we can't remove stays pending — bounded by its own TTL.
    }
    if (file !== null && file.expiresAt > now()) {
      store.register(file.code, file.expiresAt);
    }
  };

  return {
    redeem: (code) => {
      absorbCodeFile();
      if (!store.redeem(code, now())) return null;
      const token = mintPairToken();
      hashes.add(hashPairToken(token));
      flushTokens();
      return token;
    },
    accepts: (token) => hashes.has(hashPairToken(token)),
  };
};
