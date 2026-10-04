import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/**
 * Sepia-owned per-session metadata (title overrides, future fields). Lives in
 * a separate JSON file because the Devin session store is opened read-only
 * and ACP has no session-rename operation.
 */
export interface SessionMeta {
  readonly title?: string;
}

export interface MetaStore {
  readonly titleOf: (id: string) => string | undefined;
  readonly rename: (id: string, title: string) => void;
  readonly remove: (id: string) => void;
}

export const createMetaStore = (path: string): MetaStore => {
  let data: Record<string, SessionMeta> = {};

  const flush = (): void => {
    mkdirSync(dirname(path), { recursive: true });
    const tmp = `${path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(data));
    renameSync(tmp, path);
  };

  if (existsSync(path)) {
    try {
      const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
      if (typeof parsed === "object" && parsed !== null) {
        data = parsed as Record<string, SessionMeta>;
      }
    } catch {
      // A corrupt or half-written file degrades to empty rather than failing.
    }
  }

  return {
    titleOf: (id) => data[id]?.title,
    rename: (id, title) => {
      data = { ...data, [id]: { ...data[id], title } };
      flush();
    },
    remove: (id) => {
      if (!(id in data)) return;
      const next = { ...data };
      delete next[id];
      data = next;
      flush();
    },
  };
};
