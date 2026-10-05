import { useEffect } from "react";
import { Store, useStore } from "@tanstack/react-store";

/**
 * This browser's client identity — the UI is a client container, so each
 * browser/device running it is a labeled *client* with its own keypair.
 * The record persists in localStorage (`sepia:client`) and is foundational
 * for client-side pairing/sync: `publicKey` is the shareable, displayable
 * half; `secretKey` is the private material — it never leaves this browser
 * and renders masked everywhere.
 *
 * Keys generate lazily via WebCrypto (`ensureClient` — the API is async, so
 * the store starts empty on first run and fills once generation settles).
 * Ed25519 is preferred; ECDSA P-256 is the universal fallback; on a
 * non-secure context (a `http://lan` origin) `crypto.subtle` is absent and
 * the client still gets an id + random secret with `algorithm: "none"`.
 */
export type ClientAlgorithm = "Ed25519" | "ECDSA-P-256" | "none";

export interface ClientIdentity {
  /** `client_<random>` — the stable handle, kept across re-keys. */
  readonly id: string;
  /** User-editable display name (default: a friendly device guess). */
  readonly label: string;
  /** base64url of the exported raw public key — safe to share/display. */
  readonly publicKey: string;
  /**
   * The private material — the exported JWK `d` scalar (base64url), or
   * random bytes when WebCrypto is unavailable. Local + masked only.
   */
  readonly secretKey: string;
  /** Which signature scheme `publicKey`/`secretKey` belong to. */
  readonly algorithm: ClientAlgorithm;
}

const KEY = "sepia:client";

/** 0-9a-f — deterministic display alphabet for ids. */
const randomHex = (length: number): string => {
  const bytes =
    typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function"
      ? crypto.getRandomValues(new Uint8Array(Math.ceil(length / 2)))
      : Uint8Array.from({ length: Math.ceil(length / 2) }, () => Math.floor(Math.random() * 256));
  return [...bytes]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .slice(0, length);
};

export const newClientId = (): string => `client_${randomHex(12)}`;

const base64url = (bytes: ArrayBuffer | Uint8Array): string => {
  const buf = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let binary = "";
  for (const b of buf) binary += String.fromCharCode(b);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
};

/** Device-guess default label — the user can rename it in Settings → Client. */
export const defaultClientLabel = (): string => {
  if (typeof navigator === "undefined") return "Client";
  const ua = navigator.userAgent;
  if (/iphone/i.test(ua)) return "iPhone";
  if (/ipad/i.test(ua)) return "iPad";
  if (/android/i.test(ua)) return /mobile/i.test(ua) ? "Android phone" : "Android tablet";
  if (/macintosh|mac os/i.test(ua)) return "Mac";
  if (/windows/i.test(ua)) return "Windows PC";
  if (/cros/i.test(ua)) return "Chromebook";
  if (/linux/i.test(ua)) return "Linux machine";
  return "Client";
};

type ClientKeys = Pick<ClientIdentity, "algorithm" | "publicKey" | "secretKey">;

/** No usable WebCrypto — the identity still needs a secret for future pairing. */
const fallbackKeys = (): ClientKeys => ({
  algorithm: "none",
  publicKey: "",
  secretKey: base64url(
    typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function"
      ? crypto.getRandomValues(new Uint8Array(32))
      : Uint8Array.from({ length: 32 }, () => Math.floor(Math.random() * 256)),
  ),
});

/**
 * Generate a signing keypair. Ed25519 first — its keys are compact — but
 * support still varies across browsers, so any failure falls through to
 * ECDSA P-256 (universally supported). Both failing lands on `none`.
 */
const generateKeys = async (): Promise<ClientKeys> => {
  const subtle =
    typeof crypto !== "undefined" && crypto.subtle !== undefined ? crypto.subtle : undefined;
  if (subtle === undefined) return fallbackKeys();
  const attempts: ReadonlyArray<{
    params: EcKeyGenParams | { name: string };
    algorithm: ClientAlgorithm;
  }> = [
    { params: { name: "Ed25519" }, algorithm: "Ed25519" },
    { params: { name: "ECDSA", namedCurve: "P-256" }, algorithm: "ECDSA-P-256" },
  ];
  for (const { params, algorithm } of attempts) {
    try {
      // The union params widen the return — both attempts mint keypairs.
      const pair = (await subtle.generateKey(params, true, ["sign", "verify"])) as CryptoKeyPair;
      const raw = await subtle.exportKey("raw", pair.publicKey);
      const jwk = await subtle.exportKey("jwk", pair.privateKey);
      if (typeof jwk.d !== "string" || jwk.d === "") continue;
      return { algorithm, publicKey: base64url(raw), secretKey: jwk.d };
    } catch {
      // Algorithm unsupported in this engine — try the next.
    }
  }
  return fallbackKeys();
};

/** Lenient stored-record read — a malformed entry is dropped, not fatal. */
export const normalizeClient = (value: unknown): ClientIdentity | null => {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "string" || !raw.id.startsWith("client_")) return null;
  if (typeof raw.secretKey !== "string" || raw.secretKey === "") return null;
  const publicKey = typeof raw.publicKey === "string" ? raw.publicKey : "";
  const algorithm: ClientAlgorithm =
    raw.algorithm === "Ed25519" || raw.algorithm === "ECDSA-P-256" || raw.algorithm === "none"
      ? raw.algorithm
      : publicKey === ""
        ? "none"
        : "ECDSA-P-256";
  return {
    id: raw.id,
    label:
      typeof raw.label === "string" && raw.label.trim() !== ""
        ? raw.label.trim()
        : defaultClientLabel(),
    publicKey,
    secretKey: raw.secretKey,
    algorithm,
  };
};

const loadClient = (): ClientIdentity | null => {
  try {
    const raw = localStorage.getItem(KEY);
    return raw === null ? null : normalizeClient(JSON.parse(raw));
  } catch {
    return null;
  }
};

const persistClient = (client: ClientIdentity): void => {
  try {
    localStorage.setItem(KEY, JSON.stringify(client));
  } catch {
    // Storage unavailable — the identity lives for the session.
  }
};

interface ClientState {
  /** null until `ensureClient` settles on a first run — generation is async. */
  readonly client: ClientIdentity | null;
}

export const clientStore = new Store<ClientState>({ client: loadClient() });

const commitClient = (client: ClientIdentity): void => {
  persistClient(client);
  clientStore.setState(() => ({ client }));
};

let pending: Promise<ClientIdentity> | null = null;

/**
 * The lazy init — returns the stored identity, or generates + persists one
 * on first access. Concurrent callers share one in-flight generation.
 */
export const ensureClient = (): Promise<ClientIdentity> => {
  const existing = clientStore.state.client;
  if (existing !== null) return Promise.resolve(existing);
  pending ??= (async (): Promise<ClientIdentity> => {
    const client: ClientIdentity = {
      id: newClientId(),
      label: defaultClientLabel(),
      ...(await generateKeys()),
    };
    commitClient(client);
    return client;
  })().finally(() => {
    pending = null;
  });
  return pending;
};

/** Rename the client — an empty/whitespace label keeps the current one. */
export const setClientLabel = (label: string): void => {
  const trimmed = label.trim();
  const client = clientStore.state.client;
  if (trimmed === "" || client === null) return;
  commitClient({ ...client, label: trimmed });
};

/**
 * Re-key the client — a fresh keypair under the same id + label. This
 * changes the client identity as peers see it (the public key IS it), so
 * callers confirm first.
 */
export const regenerateClient = async (): Promise<ClientIdentity | null> => {
  const client = clientStore.state.client;
  if (client === null) return null;
  const next: ClientIdentity = { ...client, ...(await generateKeys()) };
  commitClient(next);
  return next;
};

/**
 * The client identity as React state — kicks off lazy generation on first
 * read, so callers just render `null` as "generating".
 */
export const useClient = (): ClientIdentity | null => {
  const client = useStore(clientStore, (state) => state.client);
  useEffect(() => {
    if (client === null) void ensureClient().catch(() => undefined);
  }, [client]);
  return client;
};

/** "mQWs9Lx3nK…8fE2pQ" — long keys shorten for display; title/copy keep the full form. */
export const truncateKey = (key: string, head = 10, tail = 8): string =>
  key.length <= head + tail + 1 ? key : `${key.slice(0, head)}…${key.slice(-tail)}`;
