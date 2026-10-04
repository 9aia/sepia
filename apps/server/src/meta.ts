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
  readonly projectIds?: ReadonlyArray<string>;
  /** Preferred spawn model for this session — applied on next attach. */
  readonly model?: string | null;
}

export interface Project {
  readonly id: string;
  readonly name: string;
}

interface MetaFile {
  sessions: Record<string, SessionMeta>;
  projects: Record<string, { name: string }>;
  /** Server-persisted app/UI config (collapse state, prefs). */
  config: Record<string, unknown>;
}

export interface MetaStore {
  readonly of: (id: string) => SessionMeta | undefined;
  readonly patch: (id: string, patch: Partial<SessionMeta>) => void;
  readonly remove: (id: string) => void;
  readonly listProjects: () => ReadonlyArray<Project>;
  readonly createProject: (name: string) => Project;
  readonly renameProject: (id: string, name: string) => boolean;
  readonly deleteProject: (id: string) => void;
  readonly config: () => Record<string, unknown>;
  readonly setConfig: (key: string, value: unknown) => void;
}

export const createMetaStore = (path: string): MetaStore => {
  let data: MetaFile = { sessions: {}, projects: {}, config: {} };

  const flush = (): void => {
    mkdirSync(dirname(path), { recursive: true });
    const tmp = `${path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(data));
    renameSync(tmp, path);
  };

  // Migrates older shapes: v1 flat map, singular projectId.
  const normalizeSession = (value: unknown): SessionMeta => {
    const rec = typeof value === "object" && value !== null ? value : {};
    const raw = rec as Record<string, unknown>;
    return {
      title: typeof raw.title === "string" ? raw.title : undefined,
      pinned: raw.pinned === true,
      projectIds: Array.isArray(raw.projectIds)
        ? raw.projectIds.filter((p): p is string => typeof p === "string")
        : typeof raw.projectId === "string"
          ? [raw.projectId]
          : [],
      model: raw.model === null || typeof raw.model === "string" ? raw.model : undefined,
    };
  };

  if (existsSync(path)) {
    try {
      const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
      if (typeof parsed === "object" && parsed !== null) {
        const record = parsed as Record<string, unknown>;
        // v2: { sessions, projects } — else v1 flat Record<sessionId, SessionMeta>.
        const rawSessions =
          typeof record.sessions === "object" && record.sessions !== null
            ? (record.sessions as Record<string, unknown>)
            : record;
        const rawProjects =
          typeof record.projects === "object" && record.projects !== null
            ? (record.projects as Record<string, { name: string }>)
            : {};
        const sessions: Record<string, SessionMeta> = {};
        for (const [id, value] of Object.entries(rawSessions)) {
          sessions[id] = normalizeSession(value);
        }
        data = {
          sessions,
          projects: rawProjects,
          config:
            typeof record.config === "object" && record.config !== null
              ? (record.config as Record<string, unknown>)
              : {},
        };
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
    config: () => data.config,
    setConfig: (key, value) => {
      data = { ...data, config: { ...data.config, [key]: value } };
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
          meta.projectIds?.includes(id) === true
            ? { ...meta, projectIds: meta.projectIds.filter((p) => p !== id) }
            : meta,
        ]),
      );
      data = { ...data, sessions, projects };
      flush();
    },
  };
};
