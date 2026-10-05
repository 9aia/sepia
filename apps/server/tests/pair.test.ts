import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import {
  createPairing,
  hashPairToken,
  mintPairCode,
  mintPairToken,
  normalizePairCode,
  PAIR_CODE_TTL_MS,
  PairingStore,
} from "../src/pair";

const tempDir = (): string => mkdtempSync(join(tmpdir(), "sepia-pair-"));

const paths = () => {
  const dir = tempDir();
  return { codeFile: join(dir, "pair-code"), tokensFile: join(dir, "tokens.json") };
};

const writeCodeFile = (path: string, code: string, expiresAt: number): void => {
  writeFileSync(path, JSON.stringify({ code, expiresAt }));
};

describe("mintPairCode / normalizePairCode", () => {
  it("mints 4-4 grouped Crockford codes (no I/L/O/U)", () => {
    for (let i = 0; i < 50; i += 1) {
      const code = mintPairCode();
      expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$/);
    }
  });

  it("normalizes case, dashes and stray characters", () => {
    expect(normalizePairCode("abcd-efgh")).toBe("ABCDEFGH");
    expect(normalizePairCode(" AB CD EF GH ")).toBe("ABCDEFGH");
    expect(normalizePairCode("ab.cd/ef-gh")).toBe("ABCDEFGH");
    expect(normalizePairCode("")).toBe("");
  });
});

describe("PairingStore", () => {
  it("redeems a minted code once — the second attempt fails", () => {
    const store = new PairingStore();
    const { code } = store.mint();
    expect(store.redeem(code)).toBe(true);
    expect(store.redeem(code)).toBe(false);
  });

  it("redeems regardless of user formatting", () => {
    const store = new PairingStore();
    const { code } = store.mint();
    expect(store.redeem(code.toLowerCase().replace("-", " "))).toBe(true);
  });

  it("expires codes after the TTL", () => {
    const store = new PairingStore();
    const now = Date.now();
    const fresh = new PairingStore();
    const { code } = fresh.mint(now);
    expect(fresh.redeem(code, now + PAIR_CODE_TTL_MS - 1)).toBe(true);

    const { code: stale } = store.mint(now);
    expect(store.redeem(stale, now + PAIR_CODE_TTL_MS + 1)).toBe(false);
  });

  it("sweeps expired codes so a stale entry can't redeem later", () => {
    const store = new PairingStore();
    const now = Date.now();
    store.register("AAAA-BBBB", now + 1_000);
    expect(store.redeem("aaaa-bbbb", now + 2_000)).toBe(false);
  });

  it("registered codes honour their own expiry, not the store TTL", () => {
    const store = new PairingStore();
    const now = Date.now();
    store.register("AAAA-BBBB", now + 60_000);
    expect(store.redeem("aaaa bbbb", now + 59_000)).toBe(true);
  });
});

describe("createPairing", () => {
  it("redeems a code written to the pair-code file and deletes the file", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeCodeFile(codeFile, "ABCD-EFGH", Date.now() + PAIR_CODE_TTL_MS);

    const token = pairing.redeem("abcd efgh");
    expect(token).toMatch(/^sepia_/);
    expect(existsSync(codeFile)).toBe(false);
    // Single-use end to end.
    expect(pairing.redeem("abcd-efgh")).toBeNull();
  });

  it("rejects an expired code file and still consumes it", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeCodeFile(codeFile, "ABCD-EFGH", Date.now() - 1);

    expect(pairing.redeem("abcd-efgh")).toBeNull();
    expect(existsSync(codeFile)).toBe(false);
  });

  it("ignores a malformed code file", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeFileSync(codeFile, "{corrupt");
    expect(pairing.redeem("abcd-efgh")).toBeNull();
    writeFileSync(codeFile, JSON.stringify({ code: "ABCD-EFGH", expiresAt: "soon" }));
    expect(pairing.redeem("abcd-efgh")).toBeNull();
  });

  it("rejects unknown codes and leaves a pending code redeemable", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeCodeFile(codeFile, "ABCD-EFGH", Date.now() + PAIR_CODE_TTL_MS);

    expect(pairing.redeem("XXXX-XXXX")).toBeNull();
    // The file was consumed but the in-memory code survives for its TTL.
    expect(pairing.redeem("abcd-efgh")).toMatch(/^sepia_/);
  });

  it("issued credentials authenticate via accepts() and persist to tokens.json", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeCodeFile(codeFile, "ABCD-EFGH", Date.now() + PAIR_CODE_TTL_MS);

    const token = pairing.redeem("abcd-efgh");
    expect(token).not.toBeNull();
    expect(pairing.accepts(token!)).toBe(true);
    expect(pairing.accepts("sepia_wrong")).toBe(false);

    // The file stores hashes, never the credential itself.
    const onDisk = JSON.parse(readFileSync(tokensFile, "utf8")) as { tokens: string[] };
    expect(onDisk.tokens).toContain(hashPairToken(token!));
    expect(readFileSync(tokensFile, "utf8")).not.toContain(token!);

    // A rebooted server over the same file still accepts the credential.
    const rebooted = createPairing({ codeFile, tokensFile });
    expect(rebooted.accepts(token!)).toBe(true);
  });

  it("a corrupt tokens file degrades to empty rather than crashing", () => {
    const { codeFile, tokensFile } = paths();
    writeFileSync(tokensFile, "{corrupt");
    const pairing = createPairing({ codeFile, tokensFile });
    expect(pairing.accepts(mintPairToken())).toBe(false);
  });
});

describe("edge cases", () => {
  it("ignores a non-object code file and never stores an un-normalizable code", () => {
    const { codeFile, tokensFile } = paths();
    const pairing = createPairing({ codeFile, tokensFile });
    writeFileSync(codeFile, JSON.stringify("just-a-string"));
    expect(pairing.redeem("abcd-efgh")).toBeNull();

    const store = new PairingStore();
    store.register("!!!", Date.now() + PAIR_CODE_TTL_MS); // normalizes to ""
    expect(store.redeem("!!!")).toBe(false);
  });

  it("skips non-string tokens in a corrupt-shaped credentials file", () => {
    const { codeFile, tokensFile } = paths();
    writeFileSync(tokensFile, JSON.stringify({ tokens: [42, hashPairToken("sepia_good"), null] }));
    const pairing = createPairing({ codeFile, tokensFile });
    expect(pairing.accepts("sepia_good")).toBe(true);
    expect(pairing.accepts("42")).toBe(false);
  });
});
