import { resolveSession, sessionKey } from "./format";

/**
 * Most-recently-opened session ids (MRU first). Client-local — recents are a
 * browsing convenience, not shared app data.
 *
 * Deliberately not a TanStack DB collection: `getRecents()` is read
 * synchronously during render and `pushRecent()` runs inside a synchronous
 * store setter, so a collection's async preload/lifecycle would add
 * complexity for no benefit. The stored JSON array also predates the DB
 * migration — a collection would need its own format migration.
 */
const KEY = "sepia:recents";
const MAX = 30;

export const getRecents = (): string[] => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is string => typeof id === "string");
  } catch {
    return [];
  }
};

export const pushRecent = (id: string): void => {
  try {
    const next = [id, ...getRecents().filter((r) => r !== id)].slice(0, MAX);
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Storage unavailable — recents just don't persist.
  }
};

/**
 * The sidebar's "Sessions" section: every stored recent that resolves, in
 * the INPUT list's order — not MRU order. Opening a session is not
 * activity, and sorting the section by open recency would teleport the
 * clicked row to the section's top on every select. Recents may also hold
 * the same session twice (bare ids from before agent-scoped keys, plus the
 * scoped one); resolution collapses both to one key.
 */
export const resolveRecentSessions = <
  T extends { readonly agent: string; readonly id: string; readonly node?: string },
>(
  sessions: ReadonlyArray<T>,
  recents: ReadonlyArray<string> = getRecents(),
): T[] => {
  const keys = new Set(
    recents
      .map((key) => resolveSession(sessions, key))
      .filter((session): session is NonNullable<typeof session> => session !== undefined)
      .map(sessionKey),
  );
  return sessions.filter((session) => keys.has(sessionKey(session)));
};
