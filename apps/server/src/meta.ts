import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { dirname } from "node:path";

/**
 * Sepia-owned per-session metadata (title overrides, pins, project
 * assignments) plus the user-defined project list. Lives in a separate JSON
 * file because the Devin session store is opened read-only and ACP has no
 * session-mutation operations.
 */
export interface SessionMeta {
  readonly title?: string;
  readonly pinned?: boolean;
  readonly projectId?: string | null;
}

export interface Project {
  readonly id: string;
  readonly name: string;
}

interface MetaFile {
  sessions: Record<string, SessionMeta>;
  projects: Record<string, { name: string }>;
}

export interface MetaStore {
  readonly of: (id: string) => SessionMeta | undefined;
  readonly patch: (id: string, patch: Partial<SessionMeta>) => void;
  readonly remove: (id: string) => void;
  readonly listProjects: () => ReadonlyArray<Project>;
  readonly createProject: (name: string) => Project;
  readonly renameProject: (id: string, name: string) => boolean;
  readonly deleteProject: (id: string) => void;
}

export const createMetaStore = (path: string): MetaStore => {
  let data: MetaFile = { sessions: {}, projects: {} };

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
        const record = parsed as Record<string, unknown>;
        if (typeof record.sessions === "object" && record.sessions !== null) {
          data = record as unknown as MetaFile;
        } else {
          // v1 format: a flat Record<sessionId, SessionMeta>.
          data = { sessions: record as Record<string, SessionMeta>, projects: {} };
        }
      }
    } catch {
      // A corrupt or half-written file degrades to empty rather than failing.
    }
  }

  const patchSession = (id: string, patch: Partial<SessionMeta>): void => {
    data = {
      ...data,
      sessions: { ...data.sessions, [id]: { ...data.sessions[id], ...patch } },
    };
    flush();
  };

  return {
    of: (id) => data.sessions[id],
    patch: patchSession,
    remove: (id) => {
      if (!(id in data.sessions)) return;
      const next = { ...data.sessions };
      delete next[id];
      data = { ...data, sessions: next };
      flush();
    },
    listProjects: () => Object.entries(data.projects).map(([id, p]) => ({ id, name: p.name })),
    createProject: (name) => {
      const project: Project = { id: `proj_${randomUUID().slice(0, 8)}`, name };
      data = { ...data, projects: { ...data.projects, [project.id]: { name } } };
      flush();
      return project;
    },
    renameProject: (id, name) => {
      if (data.projects[id] === undefined) return false;
      data = { ...data, projects: { ...data.projects, [id]: { name } } };
      flush();
      return true;
    },
    deleteProject: (id) => {
      if (!(id in data.projects)) return;
      const projects = { ...data.projects };
      delete projects[id];
      const sessions = Object.fromEntries(
        Object.entries(data.sessions).map(([sid, meta]) => [
          sid,
          meta.projectId === id ? { ...meta, projectId: null } : meta,
        ]),
      );
      data = { sessions, projects };
      flush();
    },
  };
};
