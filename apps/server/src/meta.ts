import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { dirname } from "node:path";
import type { RunSpan } from "sepia-session-control";

/**
 * Sepia-owned per-session metadata (title overrides, pins, project
 * assignments) plus the user-defined project list. Lives in a separate JSON
 * file because the Devin session store is opened read-only and ACP has no
 * session-mutation operations.
 */
export interface SessionMeta {
  readonly title?: string;
  readonly pinned?: boolean;
  readonly archived?: boolean;
  readonly projectIds?: ReadonlyArray<string>;
  /** Preferred spawn model for this session — applied on next attach. */
  readonly model?: string | null;
  /**
   * Run provenance: each `POST /api/sessions/:id/attach` appends a span
   * recording which agent ran the session on which node — the same session
   * can resume under different agents/machines (that's Sepia's purpose), so
   * the transcript's segments stay attributable.
   */
  readonly spans?: ReadonlyArray<RunSpan>;
  /**
   * Recorded on `POST /api/sessions`: a created session may not exist in the
   * agent's store yet (it flushes on first prompt), so the agent/cwd pair is
   * what lets the server still identify it after a restart.
   */
  readonly agent?: string;
  readonly cwd?: string;
  readonly createdAt?: string;
}

/**
 * Idempotent span append — a re-attach under the same agent+node is the same
 * run continuing, not a new one. Returns the input unchanged when the last
 * span already matches, so callers can skip the write.
 */
export const appendSpan = (
  spans: ReadonlyArray<RunSpan> | undefined,
  span: RunSpan,
): ReadonlyArray<RunSpan> => {
  const last = spans?.[spans.length - 1];
  if (last !== undefined && last.agent === span.agent && last.node === span.node) {
    return spans ?? [];
  }
  return [...(spans ?? []), span];
};

const isRunSpan = (value: unknown): value is RunSpan => {
  if (typeof value !== "object" || value === null) return false;
  const raw = value as Record<string, unknown>;
  return (
    typeof raw.at === "number" &&
    Number.isFinite(raw.at) &&
    typeof raw.agent === "string" &&
    raw.agent !== "" &&
    typeof raw.node === "string" &&
    raw.node !== ""
  );
};

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
  /** Every recorded session meta, keyed by session id. */
  readonly sessions: () => Readonly<Record<string, SessionMeta>>;
  readonly patch: (id: string, patch: Partial<SessionMeta>) => void;
  /** Record a run span; a no-op when it repeats the current agent+node. */
  readonly addSpan: (id: string, span: RunSpan) => void;
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
      archived: raw.archived === true,
      projectIds: Array.isArray(raw.projectIds)
        ? raw.projectIds.filter((p): p is string => typeof p === "string")
        : typeof raw.projectId === "string"
          ? [raw.projectId]
          : [],
      model: raw.model === null || typeof raw.model === "string" ? raw.model : undefined,
      spans: Array.isArray(raw.spans) ? raw.spans.filter(isRunSpan) : [],
      agent: typeof raw.agent === "string" ? raw.agent : undefined,
      cwd: typeof raw.cwd === "string" ? raw.cwd : undefined,
      createdAt: typeof raw.createdAt === "string" ? raw.createdAt : undefined,
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
    sessions: () => data.sessions,
    patch: patchSession,
    addSpan: (id, span) => {
      const spans = appendSpan(data.sessions[id]?.spans, span);
      if (spans === data.sessions[id]?.spans) return; // duplicate — skip the write
      patchSession(id, { spans });
    },
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
