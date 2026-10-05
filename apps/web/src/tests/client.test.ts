import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  clientStore,
  defaultClientLabel,
  ensureClient,
  newClientId,
  normalizeClient,
  regenerateClient,
  setClientLabel,
  truncateKey,
  type ClientIdentity,
} from "../lib/client";

const store = new Map<string, string>();

const storage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => {
    store.set(key, String(value));
  },
  removeItem: (key: string) => {
    store.delete(key);
  },
};

beforeEach(() => {
  store.clear();
  vi.stubGlobal("localStorage", storage);
  clientStore.setState(() => ({ client: null }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const VALID: ClientIdentity = {
  id: "client_abc123",
  label: "thinkpad",
  publicKey: "pub-key",
  secretKey: "sec-key",
  algorithm: "ECDSA-P-256",
};

describe("normalizeClient", () => {
  it("drops malformed records and requires a client_ id + secret", () => {
    expect(normalizeClient(null)).toBeNull();
    expect(normalizeClient("x")).toBeNull();
    expect(normalizeClient({})).toBeNull();
    expect(normalizeClient({ id: "other_1", secretKey: "s" })).toBeNull();
    expect(normalizeClient({ id: "client_1" })).toBeNull();
    expect(normalizeClient({ id: "client_1", secretKey: "" })).toBeNull();
  });

  it("reads a valid record, defaulting the label and coercing the algorithm", () => {
    expect(normalizeClient(VALID)).toEqual(VALID);
    // A missing/blank label falls back to a device guess.
    expect(normalizeClient({ ...VALID, label: "  " })?.label).not.toBe("  ");
    // A record from before the field existed derives it from the keys.
    const { algorithm: _a, ...rest } = VALID;
    expect(normalizeClient(rest)?.algorithm).toBe("ECDSA-P-256");
    expect(normalizeClient({ ...rest, publicKey: "" })?.algorithm).toBe("none");
  });
});

describe("clientStore identity", () => {
  it("ensureClient generates a client_<id> with a WebCrypto keypair and persists it", async () => {
    const client = await ensureClient();
    expect(client.id).toMatch(/^client_[0-9a-f]{12}$/);
    expect(["Ed25519", "ECDSA-P-256"]).toContain(client.algorithm);
    expect(client.publicKey).not.toBe("");
    expect(client.secretKey).not.toBe("");
    expect(client.label).not.toBe("");
    expect(clientStore.state.client).toEqual(client);
    expect(JSON.parse(store.get("sepia:client") ?? "null")).toEqual(client);
  });

  it("ensureClient is idempotent — concurrent callers share one identity", async () => {
    const [a, b] = await Promise.all([ensureClient(), ensureClient()]);
    expect(a).toEqual(b);
    expect(await ensureClient()).toEqual(a);
  });

  it("falls back to a keyless identity when WebCrypto is unavailable", async () => {
    const realCrypto = globalThis.crypto;
    vi.stubGlobal("crypto", {
      subtle: undefined,
      getRandomValues: (a: Uint8Array<ArrayBuffer>) => realCrypto.getRandomValues(a),
    });
    const client = await ensureClient();
    expect(client.algorithm).toBe("none");
    expect(client.publicKey).toBe("");
    expect(client.secretKey).not.toBe("");
  });

  it("restores a stored identity on reload instead of regenerating", async () => {
    store.set("sepia:client", JSON.stringify(VALID));
    vi.resetModules();
    const mod = await import("../lib/client");
    expect(mod.clientStore.state.client).toEqual(VALID);
    expect(await mod.ensureClient()).toEqual(VALID);
    // A corrupt stored record regenerates on first access.
    store.set("sepia:client", "{not json");
    vi.resetModules();
    const fresh = await import("../lib/client");
    expect(fresh.clientStore.state.client).toBeNull();
    expect((await fresh.ensureClient()).id).toMatch(/^client_/);
  });

  it("setClientLabel renames + persists; blank input keeps the label", async () => {
    const client = await ensureClient();
    setClientLabel("work laptop");
    expect(clientStore.state.client?.label).toBe("work laptop");
    expect(JSON.parse(store.get("sepia:client") ?? "{}")).toMatchObject({
      id: client.id,
      label: "work laptop",
    });
    setClientLabel("   ");
    expect(clientStore.state.client?.label).toBe("work laptop");
    // No client yet — the rename is a no-op, not a crash.
    clientStore.setState(() => ({ client: null }));
    setClientLabel("ghost");
    expect(clientStore.state.client).toBeNull();
  });

  it("regenerateClient re-keys under the same id + label", async () => {
    const client = await ensureClient();
    setClientLabel("work laptop");
    const next = await regenerateClient();
    expect(next).not.toBeNull();
    expect(next?.id).toBe(client.id);
    expect(next?.label).toBe("work laptop");
    expect(next?.publicKey).not.toBe(client.publicKey);
    expect(next?.secretKey).not.toBe(client.secretKey);
    expect(JSON.parse(store.get("sepia:client") ?? "{}")).toMatchObject({ id: client.id });
    // Nothing to re-key before the first ensureClient.
    clientStore.setState(() => ({ client: null }));
    expect(await regenerateClient()).toBeNull();
  });
});

describe("helpers", () => {
  it("newClientId mints client_-prefixed hex ids", () => {
    const id = newClientId();
    expect(id).toMatch(/^client_[0-9a-f]{12}$/);
    expect(newClientId()).not.toBe(id);
  });

  it("truncateKey shortens long keys, keeps short ones whole", () => {
    expect(truncateKey("short")).toBe("short");
    const long = "a".repeat(40);
    const out = truncateKey(long);
    expect(out).toBe(`${"a".repeat(10)}…${"a".repeat(8)}`);
    expect(out).toHaveLength(19);
  });

  it("defaultClientLabel has a safe fallback off-browser", () => {
    expect(typeof defaultClientLabel()).toBe("string");
  });
});
