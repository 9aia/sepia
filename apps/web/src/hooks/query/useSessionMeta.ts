import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  convertSession,
  createProject,
  deleteProject,
  listProjects,
  patchSessionMeta,
  renameProject,
  type SessionMetaPatch,
} from "../../lib/api";
import { queryKeys } from "./keys";

export const usePatchSessionMeta = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, patch }: { id: string; patch: SessionMetaPatch }) =>
      patchSessionMeta(id, patch),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};

export const useProjects = () =>
  useQuery({
    queryKey: queryKeys.projects,
    queryFn: async () => (await listProjects()).projects,
  });

export const useCreateProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => createProject(name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects });
    },
  });
};

export const useRenameProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => renameProject(id, name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects });
    },
  });
};

export const useDeleteProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteProject(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects });
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};

export const useConvertSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, agent }: { id: string; agent: string }) => convertSession(id, agent),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
    },
  });
};
