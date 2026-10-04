import { useMutation, useQueryClient } from "@tanstack/react-query";
import { resumeSession } from "../../lib/api";
import { LOCAL_NODE_ID, sessionKey } from "../../lib/format";
import { isMultiNode, nodeName, nodeTarget } from "../../lib/nodes";
import { setSelectedId } from "../../lib/store";
import { toastError, toastLoading, toastSuccess } from "../../lib/toast";
import type { SessionSummary } from "../../lib/types";
import { useAgents } from "./useAgents";
import { useNodes, usePeerDescriptors } from "./useNodes";
import { queryKeys } from "./keys";

export interface ResumeTargetNode {
  /** Registered node id; `undefined` is the machine serving this UI. */
  readonly node?: string;
  readonly label: string;
  /** Agent ids the node reports (falls back to the merged roster). */
  readonly agents: ReadonlyArray<string>;
}

/**
 * "This machine" plus every registered peer, with each node's agent roster —
 * the rows of the "Resume on…" submenu. A peer whose descriptor hasn't landed
 * yet falls back to the merged roster; a dead peer then just fails the call.
 */
export const useResumeTargets = (): ReadonlyArray<ResumeTargetNode> => {
  const { peers, self } = useNodes();
  const { data: agents = [] } = useAgents();
  const descriptors = usePeerDescriptors(peers);
  const fallbackIds = agents.map((agent) => agent.id);
  return [
    { label: "This machine", agents: self?.agents ?? fallbackIds },
    ...peers.map((peer, index) => ({
      node: peer.id,
      label: peer.name,
      agents: descriptors[index]?.agents ?? fallbackIds,
    })),
  ];
};

/** The `node` tag a session resumed onto `node` carries in merged lists. */
const resumedNodeTag = (node: string | undefined): string | undefined =>
  node !== undefined && node !== LOCAL_NODE_ID ? node : isMultiNode() ? LOCAL_NODE_ID : undefined;

/**
 * "Resume on…" — copies the session's full IR history from its owning node
 * into `agent`'s store on `node` (`undefined`/`"local"` = this machine), then
 * selects the new session. Long moves get a loading toast that resolves in
 * place.
 */
export const useResumeSession = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      session,
      agent,
      node,
    }: {
      session: SessionSummary;
      agent: string;
      node?: string;
    }) => {
      const pending = toastLoading(`Resuming session on ${nodeName(node)}…`);
      try {
        const resumed = await resumeSession(
          nodeTarget(session.node),
          session.id,
          agent,
          nodeTarget(node),
          { fromAgent: session.agent, cwd: session.cwd, title: session.title },
        );
        return { resumed, agent, node, pending };
      } catch (error) {
        toastError("Couldn't resume the session", error, pending);
        throw error;
      }
    },
    onSuccess: async ({ resumed, agent, node, pending }) => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
      setSelectedId(sessionKey({ id: resumed.id, agent, node: resumedNodeTag(node) }));
      toastSuccess(`Session resumed on ${nodeName(node)} via ${agent}`, pending);
    },
  });
};
