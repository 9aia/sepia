import { toastError, toastSuccess } from "../../lib/toast";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useLiveQuery } from "@tanstack/react-db";
import { convertSession, createProject } from "../../lib/api";
import { projectsCollection } from "../../lib/db-projects";
import type { Project } from "../../lib/types";
import { queryKeys } from "./keys";

/**
 * Live list of projects, backed by `projectsCollection` (which mirrors the
 * `queryKeys.projects` query). `data` is the projects array; `isLoading`/
 * `isReady` reflect the collection's sync status.
 */
export const useProjects = () => useLiveQuery((q) => q.from({ project: projectsCollection }));

/**
 * POST /api/projects, then write the server-shaped project into the
 * `queryKeys.projects` cache — the collection sync picks it up immediately.
 * (Direct `collection.insert` isn't used: the collection has no onInsert
 * handler, since only the server assigns project ids.)
 */
export const useCreateProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => createProject(name),
    onSuccess: ({ project }) => {
      queryClient.setQueryData<Project[]>(queryKeys.projects, (old = []) => [
        ...old.filter((p) => p.id !== project.id),
        project,
      ]);
      toastSuccess("Project created");
    },
    onError: (error) => toastError("Couldn't create the project", error),
  });
};

/**
 * Optimistic rename — the collection shows the new name immediately and the
 * onUpdate handler PATCHes /api/projects/:id; failure rolls back.
 */
export const useRenameProject = () =>
  useMutation({
    mutationFn: async ({ id, name }: { id: string; name: string }) => {
      const tx = projectsCollection.update(id, (draft) => {
        draft.name = name;
      });
      await tx.when("settled");
      toastSuccess("Project renamed");
    },
    onError: (error) => toastError("Couldn't rename the project", error),
  });

/**
 * Optimistic delete — the row leaves the list immediately and the onDelete
 * handler DELETEs /api/projects/:id; failure restores it. Sessions still get
 * invalidated because their projectIds may reference the removed project.
 */
export const useDeleteProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const tx = projectsCollection.delete(id);
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
    mutationFn: ({ id, agent, fromAgent }: { id: string; agent: string; fromAgent?: string }) =>
      convertSession(id, agent, fromAgent),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      toastSuccess("Session converted");
    },
    onError: (error) => toastError("Couldn't convert the session", error),
  });
};
