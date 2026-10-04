/**
 * Local node's bearer token, stored in localStorage (see TokenGate). Pulled
 * out of api.ts so the node registry can resolve the local target without
 * importing the whole API surface (which tests mock wholesale).
 */
const TOKEN_KEY = "sepia:token";

export const getToken = (): string | null => {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
};

export const setToken = (token: string | null): void => {
  try {
    if (token === null || token === "") localStorage.removeItem(TOKEN_KEY);
    else localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Storage unavailable (private mode); the gate keeps asking.
  }
};
