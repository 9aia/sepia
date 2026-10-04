import { useState } from "react";
import {
  Add01Icon,
  ChevronRightIcon,
  Clock01Icon,
  Delete02Icon,
  Edit02Icon,
  FolderLibraryIcon,
  MoreVerticalIcon,
  PinIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { Project, SessionSummary } from "../../lib/types";
import { formatUpdated } from "../../lib/format";
import {
  useCreateProject,
  useDeleteProject,
  useProjects,
  useRenameProject,
} from "../../hooks/query/useSessionMeta";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../ui/alert-dialog";
import { Button } from "../ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "../ui/context-menu";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { Input } from "../ui/input";
import { SessionActions } from "./SessionActions";

interface RowHandlers {
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (id: string) => void;
}

/** A session row outside the tree — same visuals + ⋯/context menus. */
function SectionSessionRow({
  session,
  selected,
  onSelect,
  onDetails,
  onDelete,
}: { readonly session: SessionSummary; readonly selected: boolean } & RowHandlers) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger className="relative block">
          <button
            type="button"
            className={`group/row flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm outline-hidden transition-colors hover:bg-accent/60 focus-visible:ring-1 focus-visible:ring-primary/50 ${
              selected ? "bg-accent/80" : ""
            }`}
            onClick={() => onSelect(session.id)}
          >
            <span className="min-w-0 flex-1 truncate font-medium" title={session.title}>
              {session.title}
            </span>
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatUpdated(session.updatedAt)}
            </span>
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="absolute top-1/2 right-2 -translate-y-1/2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
                  aria-label={`Actions for session ${session.title}`}
                  title="More actions"
                />
              }
              onClick={(event) => event.stopPropagation()}
            >
              <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <SessionActions
                Item={DropdownMenuItem as unknown as typeof ContextMenuItem}
                Separator={DropdownMenuSeparator}
                Sub={DropdownMenuSub as unknown as typeof ContextMenuSub}
                SubTrigger={DropdownMenuSubTrigger as unknown as typeof ContextMenuSubTrigger}
                SubContent={DropdownMenuSubContent as unknown as typeof ContextMenuSubContent}
                session={session}
                onSelect={onSelect}
                onDetails={onDetails}
                onRequestDelete={() => setConfirmOpen(true)}
              />
            </DropdownMenuContent>
          </DropdownMenu>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <SessionActions
            Item={ContextMenuItem}
            Separator={ContextMenuSeparator}
            Sub={ContextMenuSub}
            SubTrigger={ContextMenuSubTrigger}
            SubContent={ContextMenuSubContent}
            session={session}
            onSelect={onSelect}
            onDetails={onDetails}
            onRequestDelete={() => setConfirmOpen(true)}
          />
        </ContextMenuContent>
      </ContextMenu>
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete session?</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${session.title}" will be permanently removed from the agent's session store. This can't be undone.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => onDelete(session.id)}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function FlatSection({
  label,
  icon,
  limit,
  sessions,
  selectedId,
  ...handlers
}: {
  readonly label: string;
  readonly icon: typeof PinIcon;
  readonly limit: number;
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
} & RowHandlers) {
  const [showAll, setShowAll] = useState(false);
  if (sessions.length === 0) return null;
  const shown = showAll ? sessions : sessions.slice(0, limit);
  return (
    <section>
      <h3 className="flex items-center gap-1.5 px-3 pt-3 pb-0.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        <HugeiconsIcon icon={icon} strokeWidth={2} className="size-3.5" />
        {label}
      </h3>
      {shown.map((session) => (
        <SectionSessionRow
          key={session.id}
          session={session}
          selected={session.id === selectedId}
          {...handlers}
        />
      ))}
      {sessions.length > limit && (
        <button
          type="button"
          className="w-full px-3 py-1 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
          onClick={() => setShowAll((v) => !v)}
        >
          {showAll ? "Show less" : `Show ${sessions.length - limit} more`}
        </button>
      )}
    </section>
  );
}

interface ProjectDialogState {
  /** undefined → creating; id → renaming */
  readonly id?: string;
  readonly name: string;
}

function ProjectNameDialog({
  state,
  onClose,
  onSubmit,
}: {
  readonly state: ProjectDialogState;
  onClose: () => void;
  onSubmit: (state: ProjectDialogState, name: string) => void;
}) {
  const [name, setName] = useState(state.name);
  const dirty = name.trim() !== "" && name.trim() !== state.name;
  const isRename = state.id !== undefined;
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{isRename ? "Rename project" : "New project"}</DialogTitle>
          <DialogDescription>
            Projects are labels for grouping sessions — they don&apos;t touch the filesystem.
          </DialogDescription>
        </DialogHeader>
        <Input
          autoFocus
          value={name}
          placeholder="Project name"
          aria-label="Project name"
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") onSubmit(state, name.trim());
          }}
        />
        <DialogFooter>
          <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
          <Button disabled={!dirty} onClick={() => onSubmit(state, name.trim())}>
            {isRename ? "Rename" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ProjectsSection({
  projects,
  sessions,
  selectedId,
  ...handlers
}: {
  readonly projects: ReadonlyArray<Project>;
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
} & RowHandlers) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [dialog, setDialog] = useState<ProjectDialogState | null>(null);
  const [deleteFor, setDeleteFor] = useState<Project | null>(null);
  const createProject = useCreateProject();
  const renameProject = useRenameProject();
  const deleteProject = useDeleteProject();

  const submitName = (state: ProjectDialogState, name: string): void => {
    if (state.id === undefined) createProject.mutate(name);
    else renameProject.mutate({ id: state.id, name });
    setDialog(null);
  };

  return (
    <section>
      <div className="flex items-center justify-between px-3 pt-3 pb-0.5">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <HugeiconsIcon icon={FolderLibraryIcon} strokeWidth={2} className="size-3.5" />
          Projects
        </h3>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="New project"
          title="New project"
          onClick={() => setDialog({ name: "" })}
        >
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
        </Button>
      </div>
      {projects.length === 0 && (
        <p className="px-3 py-1 text-xs text-muted-foreground">No projects yet.</p>
      )}
      {projects.map((project) => {
        const members = sessions.filter((s) => s.projectIds.includes(project.id));
        const open = !collapsed[project.id];
        return (
          <div key={project.id} className="group/row relative">
            <button
              type="button"
              className="flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-sm transition-colors hover:bg-accent/60"
              onClick={() => setCollapsed((prev) => ({ ...prev, [project.id]: !prev[project.id] }))}
            >
              <HugeiconsIcon
                icon={ChevronRightIcon}
                strokeWidth={2}
                className={`size-3.5 shrink-0 text-muted-foreground transition-transform ${open ? "rotate-90" : ""}`}
              />
              <span className="min-w-0 flex-1 truncate font-medium" title={project.name}>
                {project.name}
              </span>
            </button>
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="absolute top-1/2 right-2 -translate-y-1/2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
                    aria-label={`Actions for project ${project.name}`}
                    title="Project actions"
                  />
                }
                onClick={(event) => event.stopPropagation()}
              >
                <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuItem onClick={() => setDialog({ id: project.id, name: project.name })}>
                  <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
                  Rename project
                </DropdownMenuItem>
                <DropdownMenuItem variant="destructive" onClick={() => setDeleteFor(project)}>
                  <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                  Delete project
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            {open && (
              <div className="ml-4 border-l border-border/50 pl-1">
                {members.length === 0 && (
                  <p className="px-3 py-1 text-xs text-muted-foreground">
                    No sessions — use &quot;Move to project&quot; on a session.
                  </p>
                )}
                {members.map((session) => (
                  <SectionSessionRow
                    key={session.id}
                    session={session}
                    selected={session.id === selectedId}
                    {...handlers}
                  />
                ))}
              </div>
            )}
          </div>
        );
      })}
      {dialog !== null && (
        <ProjectNameDialog state={dialog} onClose={() => setDialog(null)} onSubmit={submitName} />
      )}
      <AlertDialog open={deleteFor !== null} onOpenChange={(open) => !open && setDeleteFor(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete project?</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${deleteFor?.name ?? ""}" will be removed. Its sessions stay — they just lose the project label.`}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => deleteFor !== null && deleteProject.mutate(deleteFor.id)}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

interface SessionSectionsProps {
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly recentSessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (id: string) => void;
}

export function SessionSections({
  sessions,
  recentSessions,
  selectedId,
  ...handlers
}: SessionSectionsProps) {
  const { data: projects = [] } = useProjects();
  const pinned = sessions.filter((s) => s.pinned === true);
  if (pinned.length === 0 && recentSessions.length === 0 && projects.length === 0) return null;
  return (
    <div className="max-h-[45%] shrink-0 overflow-y-auto border-t border-border pb-1">
      <FlatSection
        label="Pinned"
        icon={PinIcon}
        limit={5}
        sessions={pinned}
        selectedId={selectedId}
        {...handlers}
      />
      <FlatSection
        label="Recents"
        icon={Clock01Icon}
        limit={8}
        sessions={recentSessions}
        selectedId={selectedId}
        {...handlers}
      />
      <ProjectsSection
        projects={projects}
        sessions={sessions}
        selectedId={selectedId}
        {...handlers}
      />
    </div>
  );
}
