import { createCipheriv, createDecipheriv, randomBytes, randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/**
 * Managed-server registry (Settings → Servers). Each entry is a sepia node the
 * UI can reach through this server: either directly (`http://host:port`) or via
 * an SSH local port-forward when `ssh` is configured.
 *
 * Why not better-auth: better-auth models *inbound* auth — users, sessions,
 * and (via the apiKey plugin) keys that callers present TO this app. There is
 * no plugin for persisting outbound credentials to third-party services, and
 * standing up its adapter/database layer for one JSON document would invert
 * this server's file-based storage design. Instead the registry is encrypted
 * at rest ourselves: AES-256-GCM over the JSON file, keyed by a 256-bit key in
 * `~/.config/sepia/servers.key` (mode 0600) or the `SEPIA_SERVERS_KEY` hex env.
 */
export interface ServerAuth {
  readonly type: "token" | "password";
  /** Basic-auth username — "password" type only; defaults to "sepia". */
  readonly user?: string;
  /** Bearer token or basic-auth password. Never leaves the server. */
  readonly secret: string;
}

export interface ServerSsh {
  readonly host: string;
  readonly port: number;
  readonly user: string;
  /**
   * Path to a private key on this machine, or an inline PEM the user pasted.
   * Paths are echoed back to the UI; inline keys are masked in GET responses
   * and materialized to a 0600 file only while a tunnel runs.
   */
  readonly key?: string;
}

export interface ServerEntry {
  readonly id: string;
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly auth: ServerAuth | null;
  readonly ssh: ServerSsh | null;
}

/** Validated create/update payload — same shape minus the id. */
export type ServerInput = Omit<ServerEntry, "id">;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const isPort = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 65535;

/** PEM pasted inline — everything else is treated as a filesystem path. */
export const isInlineKey = (key: string): boolean => key.includes("-----") || key.includes("\n");

const parseAuth = (
  value: unknown,
): { ok: true; auth: ServerAuth } | { ok: false; error: string } => {
  if (!isRecord(value)) return { ok: false, error: "auth must be an object" };
  if (value.type !== "token" && value.type !== "password") {
    return { ok: false, error: "auth.type must be 'token' or 'password'" };
  }
  if (typeof value.secret !== "string" || value.secret === "") {
    return { ok: false, error: "auth.secret is required" };
  }
  if (value.user !== undefined && typeof value.user !== "string") {
    return { ok: false, error: "auth.user must be a string" };
  }
  return {
    ok: true,
    auth: {
      type: value.type,
      user: typeof value.user === "string" && value.user !== "" ? value.user : undefined,
      secret: value.secret,
    },
  };
};

const parseSsh = (value: unknown): { ok: true; ssh: ServerSsh } | { ok: false; error: string } => {
  if (!isRecord(value)) return { ok: false, error: "ssh must be an object" };
  if (typeof value.host !== "string" || value.host.trim() === "") {
    return { ok: false, error: "ssh.host is required" };
  }
  if (value.host.includes(" ")) {
    return { ok: false, error: "ssh.host must not contain spaces" };
  }
  if (typeof value.user !== "string" || value.user.trim() === "") {
    return { ok: false, error: "ssh.user is required" };
  }
  if (value.key !== undefined && typeof value.key !== "string") {
    return { ok: false, error: "ssh.key must be a path or PEM string" };
  }
  const port = value.port === undefined ? 22 : value.port;
  if (!isPort(port)) return { ok: false, error: "ssh.port must be 1-65535" };
  return {
    ok: true,
    ssh: {
      host: value.host.trim(),
      port,
      user: value.user.trim(),
      key: typeof value.key === "string" && value.key !== "" ? value.key : undefined,
    },
  };
};

/**
 * Validate a request body into a ServerInput. Hostnames are restricted to
 * non-whitespace so they can't smuggle extra args into `ssh -L` specs or the
 * upstream URL.
 */
export const validateServerInput = (
  value: unknown,
): { ok: true; input: ServerInput } | { ok: false; error: string } => {
  if (!isRecord(value)) return { ok: false, error: "Expected a JSON object body" };
  if (typeof value.label !== "string" || value.label.trim() === "" || value.label.length > 100) {
    return { ok: false, error: "label must be a non-empty string (max 100)" };
  }
  if (typeof value.host !== "string" || value.host.trim() === "") {
    return { ok: false, error: "host is required" };
  }
  const host = value.host.trim();
  if (host.includes(" ") || host.includes("/")) {
    return { ok: false, error: "host must be a hostname or IP, not a URL" };
  }
  if (!isPort(value.port)) return { ok: false, error: "port must be an integer 1-65535" };

  let auth: ServerAuth | null = null;
  if (value.auth !== undefined && value.auth !== null) {
    const parsed = parseAuth(value.auth);
    if (!parsed.ok) return parsed;
    auth = parsed.auth;
  }

  let ssh: ServerSsh | null = null;
  if (value.ssh !== undefined && value.ssh !== null) {
    const parsed = parseSsh(value.ssh);
    if (!parsed.ok) return parsed;
    ssh = parsed.ssh;
  }

  return {
    ok: true,
    input: { label: value.label.trim(), host, port: value.port, auth, ssh },
  };
};

/** Lenient stored-record read — a malformed entry is dropped, not fatal. */
const normalizeEntry = (value: unknown): ServerEntry | null => {
  if (!isRecord(value) || typeof value.id !== "string" || value.id === "") return null;
  const parsed = validateServerInput(value);
  if (!parsed.ok) return null;
  return { id: value.id, ...parsed.input };
};

const MASK = "••••••••";

/** Wire shape — the secret material never crosses the API boundary. */
export const publicServer = (entry: ServerEntry): Record<string, unknown> => ({
  id: entry.id,
  label: entry.label,
  host: entry.host,
  port: entry.port,
  auth:
    entry.auth === null
      ? null
      : {
          type: entry.auth.type,
          user: entry.auth.user,
          secret: MASK,
        },
  ssh:
    entry.ssh === null
      ? null
      : {
          host: entry.ssh.host,
          port: entry.ssh.port,
          user: entry.ssh.user,
          // Paths aren't secret (the UI needs them for the edit form); an
          // inline PEM is masked like a password.
          key:
            entry.ssh.key === undefined
              ? undefined
              : isInlineKey(entry.ssh.key)
                ? MASK
                : entry.ssh.key,
          keyIsPath: entry.ssh.key === undefined ? undefined : !isInlineKey(entry.ssh.key),
        },
});

/** Authorization header for requests to the managed server. */
export const authHeader = (auth: ServerAuth | null): string | null => {
  if (auth === null) return null;
  if (auth.type === "token") return `Bearer ${auth.secret}`;
  const user = auth.user ?? "sepia";
  return `Basic ${Buffer.from(`${user}:${auth.secret}`).toString("base64")}`;
};

// --- Encrypted-at-rest persistence -------------------------------------------

interface SealedFile {
  readonly v: number;
  readonly iv: string;
  readonly tag: string;
  readonly data: string;
}

const seal = (plaintext: string, key: Buffer): SealedFile => {
  const iv = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const data = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  return {
    v: 1,
    iv: iv.toString("base64"),
    tag: cipher.getAuthTag().toString("base64"),
    data: data.toString("base64"),
  };
};

const unseal = (file: SealedFile, key: Buffer): string => {
  const decipher = createDecipheriv("aes-256-gcm", key, Buffer.from(file.iv, "base64"));
  decipher.setAuthTag(Buffer.from(file.tag, "base64"));
  return Buffer.concat([
    decipher.update(Buffer.from(file.data, "base64")),
    decipher.final(),
  ]).toString("utf8");
};

/**
 * Load (or mint) the 256-bit key. `SEPIA_SERVERS_KEY` (64 hex chars) takes
 * precedence over the key file so operators can keep it in a secret manager.
 */
const loadKey = (keyPath: string, env: NodeJS.ProcessEnv): Buffer => {
  const fromEnv = env.SEPIA_SERVERS_KEY;
  if (fromEnv !== undefined && fromEnv !== "") {
    if (!/^[0-9a-f]{64}$/i.test(fromEnv)) {
      throw new Error("SEPIA_SERVERS_KEY must be 64 hex characters");
    }
    return Buffer.from(fromEnv, "hex");
  }
  if (existsSync(keyPath)) {
    const raw = readFileSync(keyPath, "utf8").trim();
    if (/^[0-9a-f]{64}$/i.test(raw)) return Buffer.from(raw, "hex");
    throw new Error(`servers key file is corrupt: ${keyPath}`);
  }
  mkdirSync(dirname(keyPath), { recursive: true, mode: 0o700 });
  const key = randomBytes(32);
  writeFileSync(keyPath, `${key.toString("hex")}\n`, { mode: 0o600 });
  chmodSync(keyPath, 0o600); // tighten umask-loosened perms on existing file too
  return key;
};

export interface ServerStore {
  readonly list: () => ReadonlyArray<ServerEntry>;
  readonly get: (id: string) => ServerEntry | undefined;
  readonly create: (input: ServerInput) => ServerEntry;
  /**
   * Partial update. `auth`/`ssh` replace or clear (null); a `secret`/`key`
   * equal to the UI's mask placeholder keeps the stored value so the edit
   * form can round-trip masked secrets without re-entry.
   */
  readonly update: (id: string, input: ServerInput) => ServerEntry | undefined;
  readonly remove: (id: string) => boolean;
  /** Non-null when the on-disk file couldn't be decrypted — mutations throw. */
  readonly error: string | null;
}

const UNDECRYPTABLE = "servers file could not be decrypted — check the key";

export const createServerStore = (
  path: string,
  keyPath: string,
  env: NodeJS.ProcessEnv = process.env,
): ServerStore => {
  const key = loadKey(keyPath, env);
  let entries: ServerEntry[] = [];
  let error: string | null = null;

  if (existsSync(path)) {
    try {
      const sealed = JSON.parse(readFileSync(path, "utf8")) as SealedFile;
      const plaintext = unseal(sealed, key);
      const parsed = JSON.parse(plaintext) as unknown;
      if (isRecord(parsed) && Array.isArray(parsed.servers)) {
        entries = parsed.servers
          .map(normalizeEntry)
          .filter((entry): entry is ServerEntry => entry !== null);
      }
    } catch {
      // Fail closed: serve an empty registry and refuse to overwrite the
      // file — a wrong key must not destroy the stored credentials.
      error = UNDECRYPTABLE;
    }
  }

  const flush = (): void => {
    if (error !== null) throw new Error(UNDECRYPTABLE);
    mkdirSync(dirname(path), { recursive: true });
    const tmp = `${path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(seal(JSON.stringify({ servers: entries }), key)), {
      mode: 0o600,
    });
    renameSync(tmp, path);
    chmodSync(path, 0o600);
  };

  return {
    get error() {
      return error;
    },
    list: () => entries,
    get: (id) => entries.find((entry) => entry.id === id),
    create: (input) => {
      const entry: ServerEntry = {
        id: `srv_${randomUUID().replaceAll("-", "").slice(0, 12)}`,
        ...input,
      };
      entries = [...entries, entry];
      flush();
      return entry;
    },
    update: (id, input) => {
      const index = entries.findIndex((entry) => entry.id === id);
      if (index === -1) return undefined;
      const existing = entries[index];
      // The mask placeholder means "keep the stored secret" — the UI echoes
      // it back for fields the user didn't retype in the edit form.
      const auth =
        input.auth === null
          ? null
          : input.auth.secret === MASK && existing.auth !== null
            ? { ...input.auth, secret: existing.auth.secret }
            : input.auth;
      const ssh =
        input.ssh === null
          ? null
          : input.ssh.key === MASK && existing.ssh !== null
            ? { ...input.ssh, key: existing.ssh.key }
            : input.ssh;
      const next: ServerEntry = { ...input, auth, ssh, id };
      entries = entries.with(index, next);
      flush();
      return next;
    },
    remove: (id) => {
      const next = entries.filter((entry) => entry.id !== id);
      if (next.length === entries.length) return false;
      entries = next;
      flush();
      return true;
    },
  };
};
