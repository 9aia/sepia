import { settingsStore } from "./settings";

/**
 * The local node's bearer token, stored in localStorage (see TokenGate).
 * Pulled out of api.ts so the node registry can resolve the local target
 * without importing the whole API surface (which tests mock wholesale).
 *
 * Tokens are bound to the node address they were entered for: the store is
 * a map of `{ baseUrl: token }` where `""` is the serving origin (the
 * default — relative calls) and an origin like `https://thinkpad:8787` is a
 * `settings.localNodeUrl` override. `getToken()` only ever returns the
 * token whose key matches the CURRENT effective local address — a repointed
 * override resolves no token (calls 401 → the gate re-prompts), so the
 * credential can never be shipped to a host it wasn't issued for.
 */
const TOKEN_KEY = "sepia:token";

type TokenMap = Record<string, string>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** The address a token entered right now would authenticate. */
const localBase = (): string => settingsStore.state.localNodeUrl ?? "";

const writeTokens = (tokens: TokenMap): void => {
  try {
    if (Object.keys(tokens).length === 0) localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, JSON.stringify(tokens));
  } catch {
    // Storage unavailable (private mode); the gate keeps asking.
  }
};

const readTokens = (): TokenMap => {
  try {
    const raw = localStorage.getItem(TOKEN_KEY);
    if (raw === null) return {};
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      // Pre-binding stores held the bare token string.
      parsed = raw;
    }
    if (typeof parsed === "string") {
      if (parsed === "") return {};
      // Migrate by pinning the legacy token to the address the client is
      // pointed at RIGHT NOW — persist it, or the slot would follow a later
      // `localNodeUrl` change and carry the credential to a new host.
      const migrated: TokenMap = { [localBase()]: parsed };
      writeTokens(migrated);
      return migrated;
    }
    if (!isRecord(parsed)) return {};
    const tokens: TokenMap = {};
    for (const [base, token] of Object.entries(parsed)) {
      if (typeof token === "string" && token !== "") tokens[base] = token;
    }
    return tokens;
  } catch {
    return {};
  }
};

export const getToken = (): string | null => readTokens()[localBase()] ?? null;

const listeners = new Set<() => void>();

/**
 * Fires after `setToken` — the /api/events feed re-subscribes with the new
 * credential without polling localStorage.
 */
export const onTokenChange = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

export const setToken = (token: string | null): void => {
  const tokens = readTokens();
  if (token === null || token === "") delete tokens[localBase()];
  else tokens[localBase()] = token;
  writeTokens(tokens);
  for (const listener of listeners) listener();
};

/**
 * httpOnly-cookie mode: `POST /api/auth/login` set `sepia_token`
 * server-side, so no credential lives in JS/localStorage. The flag is
 * module state (not persisted): a fresh page can't know the cookie exists
 * until a call succeeds, but once any local call 401s the gate re-runs
 * the login and re-arms it.
 */
let cookieAuth = false;

export const setCookieAuth = (enabled: boolean): void => {
  cookieAuth = enabled;
  for (const listener of listeners) listener();
};

/** True when the local target authenticates via cookie instead of Bearer. */
export const isCookieAuth = (): boolean => cookieAuth;
