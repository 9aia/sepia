import { Store } from "@tanstack/react-store";

/**
 * Client-held peer credentials (Settings → Credentials) — the managed
 * store behind `PeerNode.credentialId`. Secrets live only in localStorage
 * and render masked everywhere in the UI; they never leave the machine
 * except as the bearer header on calls to the peer that owns them.
 *
 * This is the client-local store for direct peers only. `via: "gateway"`
 * peers keep no credential here — theirs lives in the node's managed-server
 * registry (lib/servers.ts), encrypted at rest server-side.
 */
export interface Credential {
  /** `cred_<random>` — peers reference it via `credentialId`. */
  readonly id: string;
  /** Display label — editable in Settings → Credentials. */
  readonly label: string;
  /** Credential kind — only bearer tokens today; the field keeps room. */
  readonly type: "token";
  /** The real secret — masked (SECRET_MASK) anywhere it displays. */
  readonly secret: string;
}

const KEY = "sepia:credentials";

const newCredentialId = (): string =>
  `cred_${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;

/** Lenient stored-record read — a malformed entry is dropped, not fatal. */
export const normalizeCredential = (value: unknown): Credential | null => {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "string" || raw.id === "") return null;
  if (typeof raw.secret !== "string" || raw.secret === "") return null;
  // Only "token" exists today; anything else is from a newer build and can't
  // be used here — drop it rather than mis-send it.
  if (raw.type !== "token" && raw.type !== undefined) return null;
  return {
    id: raw.id,
    label: typeof raw.label === "string" && raw.label.trim() !== "" ? raw.label.trim() : "Token",
    type: "token",
    secret: raw.secret,
  };
};

const loadCredentials = (): Credential[] => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .map(normalizeCredential)
      .filter((credential): credential is Credential => credential !== null);
  } catch {
    return [];
  }
};

const persistCredentials = (credentials: ReadonlyArray<Credential>): void => {
  try {
    localStorage.setItem(KEY, JSON.stringify(credentials));
  } catch {
    // Storage unavailable — credentials live for the session.
  }
};

export const credentialsStore = new Store<ReadonlyArray<Credential>>(loadCredentials());

const commitCredentials = (credentials: ReadonlyArray<Credential>): void => {
  persistCredentials(credentials);
  credentialsStore.setState(() => credentials);
};

/** Look up a credential by id — the resolution behind `credentialId`. */
export const credentialById = (id: string | undefined): Credential | null =>
  id === undefined
    ? null
    : (credentialsStore.state.find((credential) => credential.id === id) ?? null);

/** Add a credential (label falls back to "Token"); returns the new entry. */
export const addCredential = (input: {
  readonly label: string;
  readonly secret: string;
}): Credential => {
  const credential: Credential = {
    id: newCredentialId(),
    label: input.label.trim() === "" ? "Token" : input.label.trim(),
    type: "token",
    secret: input.secret,
  };
  commitCredentials([...credentialsStore.state, credential]);
  return credential;
};

/**
 * Drop a credential. Peers still referencing its id keep the (now dangling)
 * link — their calls resolve no secret and fail auth, which Settings →
 * Credentials' delete confirm warns about with the referencing-node count.
 */
export const removeCredential = (id: string): void => {
  commitCredentials(credentialsStore.state.filter((credential) => credential.id !== id));
};

/** Rename a credential — an empty/whitespace label keeps the current one. */
export const setCredentialLabel = (id: string, label: string): void => {
  const trimmed = label.trim();
  if (trimmed === "") return;
  commitCredentials(
    credentialsStore.state.map((credential) =>
      credential.id !== id ? credential : { ...credential, label: trimmed },
    ),
  );
};
