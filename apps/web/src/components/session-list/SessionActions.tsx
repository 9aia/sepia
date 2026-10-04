import {
  ArrowReloadHorizontalIcon,
  Copy01Icon,
  Delete02Icon,
  Edit02Icon,
  FolderLibraryIcon,
  FolderOpenIcon,
  InformationCircleIcon,
  PinIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../../lib/types";
import {
  useConvertSession,
  usePatchSessionMeta,
  useProjects,
} from "../../hooks/query/useSessionMeta";
import { useAgents } from "../../hooks/query/useAgents";
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
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onRequestDelete: (id: string) => void;
}) {
  const patch = usePatchSessionMeta();
  const convert = useConvertSession();
  const { data: projects = [] } = useProjects();
  const { data: agents = [] } = useAgents();
  const convertTargets = agents.filter((a) => a.id !== session.agent);
  const copy = (value: string) =>
    void navigator.clipboard.writeText(value).then(
      () => {},
      () => undefined,
    );
  const setProject = (projectId: string | null) =>
    patch.mutate({ id: session.id, patch: { projectId } });
  const inProject = session.projectId !== null && session.projectId !== undefined;
  return (
    <>
      <Item onClick={() => onSelect(session.id)}>
        <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
        Open
      </Item>
      <Item onClick={() => patch.mutate({ id: session.id, patch: { pinned: !session.pinned } })}>
        <HugeiconsIcon icon={PinIcon} strokeWidth={2} />
        {session.pinned === true ? "Unpin" : "Pin"}
      </Item>
      <Item onClick={() => onDetails(session.id, true)}>
        <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
        Rename…
      </Item>
      <Item onClick={() => onDetails(session.id, false)}>
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        Details
      </Item>
      <Separator />
      <Sub>
        <SubTrigger>
          <HugeiconsIcon icon={FolderLibraryIcon} strokeWidth={2} />
          Move to project
        </SubTrigger>
        <SubContent className="w-48">
          {projects.length === 0 && (
            <Item disabled>
              <span className="text-muted-foreground">No projects yet</span>
            </Item>
          )}
          {projects.map((project) => (
            <Item key={project.id} onClick={() => setProject(project.id)}>
              {project.name}
              {session.projectId === project.id && (
                <span className="ml-auto text-xs text-primary">✓</span>
              )}
            </Item>
          ))}
          {inProject && <Separator />}
          {inProject && <Item onClick={() => setProject(null)}>Remove from project</Item>}
        </SubContent>
      </Sub>
      {convertTargets.length > 0 && (
        <Sub>
          <SubTrigger>
            <HugeiconsIcon icon={ArrowReloadHorizontalIcon} strokeWidth={2} />
            Convert
          </SubTrigger>
          <SubContent className="w-48">
            {convertTargets.map((agent) => (
              <Item
                key={agent.id}
                onClick={() => convert.mutate({ id: session.id, agent: agent.id })}
              >
                To {agent.label}
              </Item>
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
      <Item variant="destructive" onClick={() => onRequestDelete(session.id)}>
        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
        Delete
      </Item>
    </>
  );
}
