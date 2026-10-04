import type { Collection } from "@tanstack/db";
import type { QueryClient } from "@tanstack/react-query";
import { deleteProject, renameProject } from "./api";
import { createQueryCollection } from "./db-query-collection";
import { projectKey } from "./format";
import { listAllProjects, nodeTarget } from "./nodes";
import type { Project } from "./types";
import { queryKeys } from "../hooks/query/keys";
import { queryClient } from "../hooks/query/queryClient";

const rename = async (project: Project, name: string): Promise<void> => {
  if (!(await renameProject(project.id, name, nodeTarget(project.node)))) {
    throw new Error("Rename failed");
  }
};

const remove = async (project: Project): Promise<void> => {
  if (!(await deleteProject(project.id, nodeTarget(project.node)))) {
    throw new Error("Delete failed");
  }
};

/**
 * Projects as a Query-backed collection spanning every registered node:
 * reads mirror the `queryKeys.projects` query (a merged fan-out), keys are
 * `node:id` when `node` is set so cross-node id collisions can't merge rows,
 * and `update`/`delete` route to the owning node via `mutation.original`.
 * There is no onInsert — `useCreateProject` POSTs first, then writes the
 * server-shaped project (tagged with its node) into the query cache.
 */
export const createProjectsCollection = (client: QueryClient): Collection<Project, string> =>
  createQueryCollection<Project, string>({
    id: "projects",
    queryClient: client,
    queryKey: queryKeys.projects,
    queryFn: listAllProjects,
    getKey: (project) => projectKey(project),
    onUpdate: async ({ transaction }) => {
      for (const m of transaction.mutations) await rename(m.original, m.modified.name);
      const patched = new Map(transaction.mutations.map((m) => [m.key, m.modified]));
      client.setQueryData<Project[]>(queryKeys.projects, (old = []) =>
        old.map((p) => patched.get(projectKey(p)) ?? p),
      );
    },
    onDelete: async ({ transaction }) => {
      for (const m of transaction.mutations) await remove(m.original);
      const deleted = new Set(transaction.mutations.map((m) => m.key));
      client.setQueryData<Project[]>(queryKeys.projects, (old = []) =>
        old.filter((p) => !deleted.has(projectKey(p))),
      );
    },
  });

/** App-wide projects collection, bound to the shared queryClient. */
export const projectsCollection = createProjectsCollection(queryClient);
