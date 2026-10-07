import { toast } from "sonner";
import { toastError, toastLoading, toastSuccess } from "../../lib/toast";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useLiveQuery } from "@tanstack/react-db";
import {
  convertSession,
  createProject,
  pullProject,
  pushProject,
  type TransferEndpoint,
} from "../../lib/api";
import { projectsCollection } from "../../lib/db-projects";
import { LOCAL_NODE_ID, projectKey } from "../../lib/format";
import { isMultiNode, listAllProjects, nodeTarget } from "../../lib/nodes";
import type { Project } from "../../lib/types";
import { queryKeys } from "./keys";

/**
 * Live list of projects across every registered node, backed by
 * `projectsCollection` (which mirrors the `queryKeys.projects` fan-out
 * query). `data` is the projects array; `isLoading`/`isReady` reflect the
 * collection's sync status — which can report ready before the underlying
 * fan-out resolves, so `dataAvailable` is the empty-state gate: true only
 * once the query has resolved a real array (an empty one counts).
 */
export const useProjects = () => {
  const query = useQuery({ queryKey: queryKeys.projects, queryFn: listAllProjects });
  const live = useLiveQuery((q) => q.from({ project: projectsCollection }));
  return {
    ...live,
    data: live.isReady ? live.data : (query.data ?? live.data),
    dataAvailable: query.data !== undefined,
  };
};

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

/** Progress line for the live transfer toast — "3/8 · Fix auth flow". */
const transferProgress = (frame: { data: Record<string, unknown> }): string | undefined => {
  if (typeof frame.data.title !== "string" || frame.data.title === "") return undefined;
  const index = typeof frame.data.index === "number" ? frame.data.index : "?";
  const total = typeof frame.data.total === "number" ? frame.data.total : "?";
  return `${index}/${total} · ${frame.data.title}`;
};

/**
 * Push `project` to another node — `POST /api/projects/:id/push` on the
 * owning node, which streams the bundle to `endpoint` (resolved by the
 * caller via lib/transfer.ts). `label` is the destination's display name.
 * Progress rides a loading toast; the events feed refreshes both lists.
 */
export const usePushProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      project,
      endpoint,
      label,
    }: {
      project: Project;
      endpoint: TransferEndpoint;
      label: string;
    }) => {
      const pending = toastLoading(`Pushing "${project.name}" to ${label}…`);
      try {
        const summary = await pushProject(
          project.id,
          endpoint,
          (frame) => {
            const progress = transferProgress(frame);
            if (frame.event === "session" && progress !== undefined) {
              // sonner updates a toast in place by id — the description
              // carries the per-session progress line.
              toast.loading(`Pushing "${project.name}" to ${label}…`, {
                id: pending,
                description: progress,
              });
            }
          },
          nodeTarget(project.node),
        );
        return { summary, pending, label };
      } catch (error) {
        toastError(`Couldn't push "${project.name}"`, error, pending);
        throw error;
      }
    },
    onSuccess: ({ summary, pending, label }) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects });
      const skipped = summary.skipped.length;
      toastSuccess(
        `Pushed ${summary.imported.length} session(s) to ${label}` +
          (skipped > 0 ? ` (${skipped} skipped)` : ""),
        pending,
      );
    },
  });
};

/**
 * Pull a project from a peer onto this machine — `POST /api/projects/pull`
 * on the local node, which fetches the peer's `export` with the resolved
 * credential and imports it. Pull = clone when the project id is new here,
 * update when it already exists (import is idempotent by id).
 */
export const usePullProject = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      endpoint,
      remoteProjectId,
      label,
    }: {
      endpoint: TransferEndpoint;
      remoteProjectId: string;
      label: string;
    }) => {
      const pending = toastLoading(`Pulling "${remoteProjectId}" from ${label}…`);
      try {
        const summary = await pullProject(
          { source: endpoint, project: remoteProjectId },
          undefined,
          nodeTarget(undefined),
        );
        return { summary, pending, label };
      } catch (error) {
        toastError(`Couldn't pull from ${label}`, error, pending);
        throw error;
      }
    },
    onSuccess: ({ summary, pending, label }) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects });
      const skipped = summary.skipped.length;
      toastSuccess(
        `Pulled "${summary.project.name}" from ${label} — ${summary.imported.length} session(s)` +
          (skipped > 0 ? ` (${skipped} skipped)` : ""),
        pending,
      );
    },
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
