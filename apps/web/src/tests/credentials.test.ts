import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import {
  addCredential,
  credentialById,
  credentialsStore,
  normalizeCredential,
  removeCredential,
  setCredentialLabel,
  type Credential,
} from "../lib/credentials";

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
  credentialsStore.setState(() => []);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Re-import both modules fresh so their stores re-run load() against `store`. */
const loadFresh = async (): Promise<typeof import("../lib/nodes")> => {
  vi.resetModules();
  return import("../lib/nodes");
};

describe("normalizeCredential", () => {
  it("drops malformed entries and requires id + secret", () => {
    expect(normalizeCredential(null)).toBeNull();
    expect(normalizeCredential("x")).toBeNull();
    expect(normalizeCredential({})).toBeNull();
    expect(normalizeCredential({ id: "cred_1", secret: "s", label: "l" })).toEqual({
      id: "cred_1",
      label: "l",
      type: "token",
      secret: "s",
    });
    expect(normalizeCredential({ id: "", secret: "s" })).toBeNull();
    expect(normalizeCredential({ id: "cred_1" })).toBeNull();
    expect(normalizeCredential({ id: "cred_1", secret: "" })).toBeNull();
    // An unknown type from a newer build drops rather than mis-sends.
    expect(normalizeCredential({ id: "cred_1", secret: "s", type: "password" })).toBeNull();
  });

  it("falls back to a generic label on missing/blank labels", () => {
    expect(normalizeCredential({ id: "cred_1", secret: "s" })?.label).toBe("Token");
    expect(normalizeCredential({ id: "cred_1", secret: "s", label: "  " })?.label).toBe("Token");
    expect(normalizeCredential({ id: "cred_1", secret: "s", label: 42 })?.label).toBe("Token");
  });
});

describe("credentialsStore CRUD", () => {
  it("addCredential files a cred_-prefixed entry and persists it", () => {
    const credential = addCredential({ label: "thinkpad", secret: "tok-1" });
    expect(credential.id).toMatch(/^cred_[0-9a-f]{12}$/);
    expect(credential).toEqual({
      id: credential.id,
      label: "thinkpad",
      type: "token",
      secret: "tok-1",
    });
    expect(credentialById(credential.id)).toEqual(credential);
    expect(credentialById("cred_nope")).toBeNull();
    expect(credentialById(undefined)).toBeNull();

    const persisted = JSON.parse(store.get("sepia:credentials") ?? "[]") as Credential[];
    expect(persisted).toEqual([credential]);
  });

  it("blank labels fall back to 'Token'", () => {
    expect(addCredential({ label: "  ", secret: "s" }).label).toBe("Token");
  });

  it("removeCredential drops the entry and repersists", () => {
    const a = addCredential({ label: "a", secret: "sa" });
    const b = addCredential({ label: "b", secret: "sb" });
    removeCredential(a.id);
    expect(credentialsStore.state.map((c) => c.id)).toEqual([b.id]);
    expect(
      (JSON.parse(store.get("sepia:credentials") ?? "[]") as Credential[]).map((c) => c.id),
    ).toEqual([b.id]);
    // Unknown id is a no-op.
    removeCredential("cred_nope");
    expect(credentialsStore.state).toHaveLength(1);
  });

  it("setCredentialLabel renames and ignores blank input", () => {
    const credential = addCredential({ label: "old", secret: "s" });
    setCredentialLabel(credential.id, "new name");
    expect(credentialById(credential.id)?.label).toBe("new name");
    setCredentialLabel(credential.id, "   ");
    expect(credentialById(credential.id)?.label).toBe("new name");
    setCredentialLabel("cred_nope", "ghost");
    expect(credentialsStore.state).toHaveLength(1);
  });

  it("survives a reload — stored entries normalize back into the store", async () => {
    store.set(
      "sepia:credentials",
      JSON.stringify([
        { id: "cred_a", label: "a", type: "token", secret: "sa" },
        { id: "", secret: "drop" },
        "garbage",
      ]),
    );
    vi.resetModules();
    const mod = await import("../lib/credentials");
    expect(mod.credentialsStore.state).toEqual([
      { id: "cred_a", label: "a", type: "token", secret: "sa" },
    ]);
  });
});

describe("peer token → credential migration", () => {
  it("a legacy inline token files a credential named after the peer and links it", async () => {
    store.set(
      "sepia:nodes",
      JSON.stringify([
        { id: "node_1", name: "thinkpad", url: "http://tp:8787", token: "legacy-tok" },
      ]),
    );
    const nodes = await loadFresh();
    const credentials = (await import("../lib/credentials")).credentialsStore;

    const peer = nodes.nodesStore.state.peers[0];
    expect(peer?.credentialId).toMatch(/^cred_/);
    // No raw secret remains on the peer record — in memory or persisted.
    expect("token" in (peer ?? {})).toBe(false);
    const credential = credentials.state.find((c) => c.id === peer?.credentialId);
    expect(credential).toMatchObject({ label: "thinkpad", type: "token", secret: "legacy-tok" });
    // The migrated link + dropped token are written back (one-time upgrade).
    const persisted = JSON.parse(store.get("sepia:nodes") ?? "[]") as Record<string, unknown>[];
    expect("token" in persisted[0]!).toBe(false);
    expect(persisted[0]?.credentialId).toBe(peer?.credentialId);
    // …and the link resolves for calls.
    expect(nodes.peerTarget(peer!).token).toBe("legacy-tok");
  });

  it("the alias wins as the credential label; gateway and linked records don't migrate", async () => {
    store.set(
      "sepia:nodes",
      JSON.stringify([
        { id: "n1", name: "one", url: "http://a", token: "t1", alias: "desk" },
        { id: "n2", name: "two", url: "http://b", token: "t2", credentialId: "cred_x" },
        { id: "n3", name: "three", url: "http://c", token: "t3", via: "gateway", serverId: "s1" },
        { id: "n4", name: "four", url: "http://d" },
      ]),
    );
    const nodes = await loadFresh();
    const credentials = (await import("../lib/credentials")).credentialsStore;
    const peers = nodes.nodesStore.state.peers;

    // n1 migrated, labelled with the alias.
    const migrated = credentials.state.find((c) => c.id === peers[0]?.credentialId);
    expect(migrated).toMatchObject({ label: "desk", secret: "t1" });
    // n2 already linked — no duplicate credential, link untouched.
    expect(peers[1]?.credentialId).toBe("cred_x");
    // n3 is gateway — its credential was never client-held; nothing created.
    expect(peers[2]?.credentialId).toBeUndefined();
    // n4 has no secret to migrate.
    expect(peers[3]?.credentialId).toBeUndefined();
    expect(credentials.state).toHaveLength(1);
  });
});
