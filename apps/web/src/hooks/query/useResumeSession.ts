import { useMutation, useQueryClient } from "@tanstack/react-query";
import { resumeSession } from "../../lib/api";
import { LOCAL_NODE_ID, sessionKey } from "../../lib/format";
import { isMultiNode, nodeName, nodeTarget } from "../../lib/nodes";
import { resumeTargets, type ResumeTargetNode } from "../../lib/resume";
import { setSelectedId } from "../../lib/store";
import { toastError, toastLoading, toastSuccess } from "../../lib/toast";
import type { SessionSummary } from "../../lib/types";
import { useAgents } from "./useAgents";
import { useNodes, usePeerDescriptors } from "./useNodes";
import { queryKeys } from "./keys";

export type { ResumeTargetNode } from "../../lib/resume";

/**
 * "This machine" plus every registered peer, with each node's agent roster —
 * the rows of the "Resume on…" submenu, filtered for `session` (its own
 * node+agent pair drops out). A peer whose descriptor hasn't landed yet
 * falls back to the merged roster; a dead peer then just fails the call.
 * Empty when no peers are registered — the menu hides.
 */
export const useResumeTargets = (
  session: Pick<SessionSummary, "agent" | "node"> | undefined,
): ReadonlyArray<ResumeTargetNode> => {
  const { peers, self } = useNodes();
  const { data: agents = [] } = useAgents();
  const descriptors = usePeerDescriptors(peers);
  const fallbackIds = agents.map((agent) => agent.id);
  return resumeTargets(
    [
      { label: "This machine", agents: self?.agents ?? fallbackIds },
      ...peers.map((peer, index) => ({
        node: peer.id,
        label: peer.name,
        agents: descriptors[index]?.agents ?? fallbackIds,
      })),
    ],
    session,
  );
};

/** The `node` tag a session resumed onto `node` carries in merged lists. */
const resumedNodeTag = (node: string | undefined): string | undefined =>
  node !== undefined && node !== LOCAL_NODE_ID ? node : isMultiNode() ? LOCAL_NODE_ID : undefined;

/**
 * "Resume on…" — copies the session's full IR from its owning node
 * (`/export`, falling back to paged `/history` on older nodes) into
 * `agent`'s store on `node` (`undefined`/`"local"` = this machine), then
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
