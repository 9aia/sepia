/**
 * Most-recently-opened session ids (MRU first). Client-local — recents are a
 * browsing convenience, not shared app data.
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
