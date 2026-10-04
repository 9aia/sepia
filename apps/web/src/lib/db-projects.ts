import type { Collection } from "@tanstack/db";
import type { QueryClient } from "@tanstack/react-query";
import { deleteProject, listProjects, renameProject } from "./api";
import { createQueryCollection } from "./db-query-collection";
import type { Project } from "./types";
import { queryKeys } from "../hooks/query/keys";
import { queryClient } from "../hooks/query/queryClient";

const fetchProjects = async (): Promise<Project[]> => (await listProjects()).projects;

const rename = async (id: string, name: string): Promise<void> => {
  if (!(await renameProject(id, name))) throw new Error("Rename failed");
};

const remove = async (id: string): Promise<void> => {
  if (!(await deleteProject(id))) throw new Error("Delete failed");
};

/**
 * Projects as a Query-backed collection: reads mirror the
 * `queryKeys.projects` query, `update`/`delete` apply optimistically and are
 * persisted through PATCH/DELETE /api/projects/:id. After a successful write
 * the cached rows converge so the synced base matches once the optimistic
 * overlay releases. There is no onInsert — `useCreateProject` POSTs first,
 * then writes the server-shaped project into the query cache (which syncs
 * into the collection).
 */
export const createProjectsCollection = (client: QueryClient): Collection<Project, string> =>
  createQueryCollection<Project, string>({
    id: "projects",
    queryClient: client,
    queryKey: queryKeys.projects,
    queryFn: fetchProjects,
    getKey: (project) => project.id,
    onUpdate: async ({ transaction }) => {
      for (const m of transaction.mutations) await rename(m.key, m.modified.name);
      client.setQueryData<Project[]>(queryKeys.projects, (old = []) =>
        old.map((p) => transaction.mutations.find((m) => m.key === p.id)?.modified ?? p),
      );
    },
    onDelete: async ({ transaction }) => {
      for (const m of transaction.mutations) await remove(m.key);
      const deleted = new Set(transaction.mutations.map((m) => m.key));
      client.setQueryData<Project[]>(queryKeys.projects, (old = []) =>
        old.filter((p) => !deleted.has(p.id)),
      );
    },
  });

/** App-wide projects collection, bound to the shared queryClient. */
export const projectsCollection = createProjectsCollection(queryClient);
