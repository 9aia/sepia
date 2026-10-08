import {
  Add01Icon,
  ArchiveIcon,
  ArrowReloadHorizontalIcon,
  Copy01Icon,
  Delete02Icon,
  Edit02Icon,
  FolderLibraryIcon,
  FolderOpenIcon,
  Globe02Icon,
  InformationCircleIcon,
  PinIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../../lib/types";
import { nodeKey, projectKey, sessionKey } from "../../lib/format";
import { setNewProjectFor } from "../../lib/store";
import { usePatchSessionMeta } from "../../hooks/query/useSessionMeta";
import { useConvertSession, useProjects } from "../../hooks/query/useProjects";
import { useAgents } from "../../hooks/query/useAgents";
import { useResumeSession, useResumeTargets } from "../../hooks/query/useResumeSession";
import type {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "../ui/context-menu";

/**
 * Shared action items used by both the context menu and the ⋯ dropdown —
 * pass the DropdownMenu/ContextMenu parts in as `Item`, `Separator` and
 * `Sub` so the same list renders in either menu.
 */
export function SessionActions({
  Item,
  Separator,
  Sub,
  SubTrigger,
  SubContent,
  session,
  hideOpen = false,
  onSelect,
  onDetails,
  onRequestDelete,
}: {
  readonly Item: typeof ContextMenuItem;
  readonly Separator: typeof ContextMenuSeparator;
  readonly Sub: typeof ContextMenuSub;
  readonly SubTrigger: typeof ContextMenuSubTrigger;
  readonly SubContent: typeof ContextMenuSubContent;
  readonly session: SessionSummary;
  /** Already viewing the session — the Open item is noise. */
  readonly hideOpen?: boolean;
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onRequestDelete: (session: SessionSummary) => void;
}) {
  const patch = usePatchSessionMeta();
  const convert = useConvertSession();
  const resume = useResumeSession();
  // Already filtered for this session's own pair; empty in single-node mode.
  const resumeNodes = useResumeTargets(session);
  const { data: projects, dataAvailable: projectsLoaded } = useProjects();
  const { data: agents = [] } = useAgents();
  const convertTargets = agents.filter((a) => a.id !== session.agent);
  const agentLabel = (id: string): string => agents.find((a) => a.id === id)?.label ?? id;
  // Projects are node-local — only offer ones living on this session's node.
  const sameNodeProjects = projects.filter(
    (project) => nodeKey(project.node) === nodeKey(session.node),
  );
  const copy = (value: string) =>
    void navigator.clipboard.writeText(value).then(
      () => {},
      () => undefined,
    );
  const toggleProject = (key: string) => {
    const ids = session.projectIds.includes(key)
      ? session.projectIds.filter((p) => p !== key)
      : [...session.projectIds, key];
    patch.mutate({
      id: session.id,
      agent: session.agent,
      node: session.node,
      patch: { projectIds: ids },
    });
  };
  return (
    <>
      {!hideOpen && (
        <Item onClick={() => onSelect(sessionKey(session))}>
          <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
          Open
        </Item>
      )}
      <Item
        onClick={() =>
          patch.mutate({
            id: session.id,
            agent: session.agent,
            node: session.node,
            patch: { pinned: !session.pinned },
          })
        }
      >
        <HugeiconsIcon icon={PinIcon} strokeWidth={2} />
        {session.pinned === true ? "Unpin" : "Pin"}
      </Item>
      <Item
        onClick={() =>
          patch.mutate({
            id: session.id,
            agent: session.agent,
            node: session.node,
            patch: { archived: !session.archived },
          })
        }
      >
        <HugeiconsIcon icon={ArchiveIcon} strokeWidth={2} />
        {session.archived === true ? "Unarchive" : "Archive"}
      </Item>
      <Item onClick={() => onDetails(sessionKey(session), true)}>
        <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
        Rename…
      </Item>
      <Item onClick={() => onDetails(sessionKey(session), false)}>
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        Details
      </Item>
      <Separator />
      <Sub>
        <SubTrigger>
          <HugeiconsIcon icon={FolderLibraryIcon} strokeWidth={2} />
          Projects…
        </SubTrigger>
        <SubContent className="w-52">
          <Item onClick={() => setNewProjectFor(sessionKey(session))}>
            <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
            New project…
          </Item>
          {sameNodeProjects.length > 0 && <Separator />}
          {projectsLoaded && sameNodeProjects.length === 0 && (
            <Item disabled>
              <span className="text-muted-foreground">No projects yet</span>
            </Item>
          )}
          {sameNodeProjects.map((project) => (
            <Item
              key={projectKey(project)}
              closeOnClick={false}
              onClick={() => toggleProject(projectKey(project))}
            >
              {project.name}
              {session.projectIds.includes(projectKey(project)) && (
                <span className="ml-auto text-xs text-primary">✓</span>
              )}
            </Item>
          ))}
        </SubContent>
      </Sub>
      {convertTargets.length > 0 && (
        <Sub>
          <SubTrigger>
            <HugeiconsIcon icon={ArrowReloadHorizontalIcon} strokeWidth={2} />
            Convert to…
          </SubTrigger>
          <SubContent className="w-48">
            {convertTargets.map((agent) => (
              <Item
                key={agent.id}
                onClick={() =>
                  convert.mutate({
                    id: session.id,
                    agent: agent.id,
                    fromAgent: session.agent,
                    node: session.node,
                  })
                }
              >
                {agent.label}
              </Item>
            ))}
          </SubContent>
        </Sub>
      )}
      {resumeNodes.length > 0 && (
        <Sub>
          <SubTrigger>
            <HugeiconsIcon icon={Globe02Icon} strokeWidth={2} />
            Resume on…
          </SubTrigger>
          <SubContent className="w-44">
            {resumeNodes.map((entry) => (
              <Sub key={entry.node ?? "local"}>
                <SubTrigger>{entry.label}</SubTrigger>
                <SubContent className="w-40">
                  {entry.agents.length === 0 && (
                    <Item disabled>
                      <span className="text-muted-foreground">No other agents</span>
                    </Item>
                  )}
                  {entry.agents.map((agentId) => (
                    <Item
                      key={agentId}
                      disabled={resume.isPending}
                      onClick={() => resume.mutate({ session, agent: agentId, node: entry.node })}
                    >
                      {agentLabel(agentId)}
                    </Item>
                  ))}
                </SubContent>
              </Sub>
            ))}
          </SubContent>
        </Sub>
      )}
      <Item onClick={() => copy(session.id)}>
        <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
        Copy session ID
      </Item>
      <Item onClick={() => copy(session.cwd)}>
        <HugeiconsIcon icon={Copy01Icon} strokeWidth={2} />
        Copy path
      </Item>
      <Separator />
      <Item variant="destructive" onClick={() => onRequestDelete(session)}>
        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
        Delete
      </Item>
    </>
  );
}
