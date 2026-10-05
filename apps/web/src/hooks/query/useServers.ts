import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { getNode } from "../../lib/api";
import { nodesStore, removePeer } from "../../lib/nodes";
import {
  createServer,
  deleteServer,
  listServers,
  serverTarget,
  updateServer,
  type ManagedServer,
  type ServerInput,
} from "../../lib/servers";
import { toastError, toastSuccess } from "../../lib/toast";
import { queryKeys } from "./keys";

/** The managed-server registry — always queried against the local node. */
export const useServers = () =>
  useQuery({ queryKey: queryKeys.servers, queryFn: listServers, retry: 1 });

/**
 * Per-server reachability for the Settings list. The probe travels through
 * `/api/servers/:id/proxy/api/node`, so SSH-tunnelled entries report up only
 * once their forward is live (it's started lazily on the first call).
 */
export const useServerStatuses = (
  servers: ReadonlyArray<ManagedServer>,
): ReadonlyArray<boolean | undefined> =>
  useQueries({
    queries: servers.map((server) => ({
      queryKey: [...queryKeys.servers, "status", server.id],
      queryFn: async () => {
        await getNode(serverTarget(server));
        return true;
      },
      retry: 1,
      refetchInterval: 15_000,
      staleTime: 10_000,
    })),
  }).map((result) => (result.isPending ? undefined : result.isSuccess));

export const useCreateServer = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: ServerInput) => createServer(input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.servers });
      toastSuccess("Server added");
    },
    // Callers render mutation.error inline — no toast here.
  });
};

export const useUpdateServer = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: ServerInput }) => updateServer(id, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.servers });
      toastSuccess("Server updated");
    },
  });
};

export const useDeleteServer = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteServer(id),
    onSuccess: (_data, id) => {
      // A gateway peer's credential lived in this entry — drop the peer too,
      // else its /api/gateway/<id> target would just 404 on every call.
      for (const peer of nodesStore.state.peers) {
        if (peer.via === "gateway" && peer.serverId === id) removePeer(peer.id);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.servers });
      toastSuccess("Server removed");
    },
    onError: (error) => toastError("Couldn't remove the server", error),
  });
};
