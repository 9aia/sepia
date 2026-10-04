import { toastError, toastSuccess } from "../../lib/toast";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useLiveQuery } from "@tanstack/react-db";
import { convertSession, createProject } from "../../lib/api";
import { projectsCollection } from "../../lib/db-projects";
import { LOCAL_NODE_ID, projectKey } from "../../lib/format";
import { isMultiNode, nodeTarget } from "../../lib/nodes";
import type { Project } from "../../lib/types";
import { queryKeys } from "./keys";

/**
 * Live list of projects across every registered node, backed by
 * `projectsCollection` (which mirrors the `queryKeys.projects` fan-out
 * query). `data` is the projects array; `isLoading`/`isReady` reflect the
 * collection's sync status.
 */
export const useProjects = () => useLiveQuery((q) => q.from({ project: projectsCollection }));

/** The `node` tag a freshly created project carries in merged lists. */
const createdNodeTag = (node: string | undefined): string | undefined =>
  node !== undefined && node !== LOCAL_NODE_ID ? node : isMultiNode() ? LOCAL_NODE_ID : undefined;

/**
 * POST /api/projects on the target node (default: local), then write the
 * server-shaped project — tagged with its node — into the
 * `queryKeys.projects` cache so the collection sync picks it up.
 */
export const useCreateProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ name, node }: { name: string; node?: string }) => {
      const result = await createProject(name, nodeTarget(node));
      // Tag with the registry id so the row's collection key matches what
      // the next merged fetch produces.
      return { project: { ...result.project, node: createdNodeTag(node) } };
    },
    onSuccess: ({ project }) => {
      queryClient.setQueryData<Project[]>(queryKeys.projects, (old = []) => [
        ...old.filter((p) => projectKey(p) !== projectKey(project)),
        project,
      ]);
      toastSuccess("Project created");
    },
    onError: (error) => toastError("Couldn't create the project", error),
  });
};

/**
 * Optimistic rename — `key` is the collection key (`node:id` or bare `id`),
 * and the collection's onUpdate routes the PATCH to the owning node.
 */
export const useRenameProject = () =>
  useMutation({
    mutationFn: async ({ key, name }: { key: string; name: string }) => {
      const tx = projectsCollection.update(key, (draft) => {
        draft.name = name;
      });
      await tx.when("settled");
      toastSuccess("Project renamed");
    },
    onError: (error) => toastError("Couldn't rename the project", error),
  });

/**
 * Optimistic delete — `key` is the collection key; the row leaves the list
 * immediately and the onDelete handler routes DELETE to the owning node.
 * Sessions still get invalidated because their projectIds may reference the
 * removed project.
 */
export const useDeleteProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (key: string) => {
      const tx = projectsCollection.delete(key);
      await tx.when("settled");
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      toastSuccess("Project deleted");
    },
    onError: (error) => toastError("Couldn't delete the project", error),
  });
};

export const useConvertSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      agent,
      fromAgent,
      node,
    }: {
      id: string;
      agent: string;
      fromAgent?: string;
      node?: string;
    }) => convertSession(id, agent, fromAgent, nodeTarget(node)),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      toastSuccess("Session converted");
    },
    onError: (error) => toastError("Couldn't convert the session", error),
  });
};
