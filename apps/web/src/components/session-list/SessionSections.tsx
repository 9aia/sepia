import { cn } from "cn";
import { useMemo, useState, type RefObject } from "react";
import { useLiveQuery } from "@tanstack/react-db";
import { eq } from "@tanstack/db";
import {
  Add01Icon,
  ChevronRightIcon,
  Delete02Icon,
  Edit02Icon,
  FolderLibraryIcon,
  InformationCircleIcon,
  MoreVerticalIcon,
  PinIcon,
  PinOffIcon,
  UserAdd01Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { Project, SessionSummary } from "../../lib/types";
import { formatUpdated, isLocalNode, nodeKey, projectKey, sessionKey } from "../../lib/format";
import { useCreateSession } from "../../hooks/query/useCreateSession";
import { useNodeLabel, useNodes } from "../../hooks/query/useNodes";
import { useUiState } from "../../hooks/query/useConfig";
import { useAgents } from "../../hooks/query/useAgents";
import { focusNode, resolveCreateTarget, useFocus } from "../../lib/focus";
import { defaultAgentFor, defaultCwdFor, recentCwdFor, settingsStore } from "../../lib/settings";
import { sidebarSectionLabel, sidebarSectionLimit } from "../../lib/sidebar";
import { modelArgsFor } from "../../lib/models";
import { useStore } from "@tanstack/react-store";
import { sessionsCollection } from "../../lib/db";
import { usePatchSessionMeta } from "../../hooks/query/useSessionMeta";
import {
  useCreateProject,
  useDeleteProject,
  useProjects,
  useRenameProject,
} from "../../hooks/query/useProjects";
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
import { ScrollArea } from "../ui/scroll-area";
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
import { LockMark } from "./LockMark";
import { NodeBadge } from "./NodeBadge";
import { SessionActions } from "./SessionActions";
import { SessionTree } from "./SessionTree";

interface RowHandlers {
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (session: SessionSummary) => void;
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
  const patch = usePatchSessionMeta();
  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger className="group/row relative block">
          <button
            type="button"
            className={`flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring ${
              selected ? "bg-accent/80" : ""
            }`}
            onClick={() => onSelect(sessionKey(session))}
          >
            <span
              className={`min-w-0 flex-1 truncate font-medium ${session.locked ? "opacity-60" : ""}`}
              title={session.title}
            >
              {session.parentSessionId !== undefined && session.parentSessionId !== "" && (
                <span className="text-muted-foreground" title="Sub-agent session">
                  ↳{" "}
                </span>
              )}
              {session.title}
            </span>
            <LockMark session={session} />
            <NodeBadge node={session.node} />
            <span className="shrink-0 text-xs text-muted-foreground">
              {formatUpdated(session.updatedAt)}
            </span>
          </button>
          <Button
            variant="ghost"
            size="icon-xs"
            className="absolute top-1/2 right-8 -translate-y-1/2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
            aria-label={session.pinned === true ? "Unpin session" : "Pin session"}
            title={session.pinned === true ? "Unpin" : "Pin"}
            onClick={(event) => {
              event.stopPropagation();
              patch.mutate({
                id: session.id,
                agent: session.agent,
                node: session.node,
                patch: { pinned: session.pinned !== true },
              });
            }}
          >
            <HugeiconsIcon icon={session.pinned === true ? PinOffIcon : PinIcon} strokeWidth={2} />
          </Button>
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
            <AlertDialogAction variant="destructive" onClick={() => onDelete(session)}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/** Section header: collapses content; the chevron is visible when closed, else on hover. */
export function SectionHeader({
  label,
  open,
  onToggle,
  action,
}: {
  readonly label: string;
  readonly open: boolean;
  onToggle: () => void;
  readonly action?: React.ReactNode;
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger className="block">
        <div className="flex items-center justify-between px-3 pt-6 pb-1.5">
          <button
            type="button"
            onClick={onToggle}
            className="flex items-center gap-1 rounded-md py-0.5 text-left text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
            aria-expanded={open}
          >
            {label}
            <HugeiconsIcon
              icon={ChevronRightIcon}
              strokeWidth={2}
              className={`size-3.5 transition-all ${
                open ? "rotate-90 opacity-0 group-hover/section:opacity-100" : ""
              }`}
            />
          </button>
          <span className="opacity-0 transition-opacity group-hover/section:opacity-100">
            {action}
          </span>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onClick={onToggle}>
          <HugeiconsIcon icon={ChevronRightIcon} strokeWidth={2} />
          {open ? "Collapse" : "Expand"} section
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

export function FlatSection({
  label,
  sectionKey,
  limit,
  sessions,
  selectedId,
  action,
  ...handlers
}: {
  readonly label: string;
  readonly sectionKey: string;
  readonly limit: number;
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  readonly action?: React.ReactNode;
} & RowHandlers) {
  const [showAll, setShowAll] = useState(false);
  const [open, setOpen] = useUiState(`ui.section.${sectionKey}`, true);
  if (sessions.length === 0) return null;
  const shown = showAll ? sessions : sessions.slice(0, limit);
  return (
    <section className="group/section">
      <SectionHeader
        label={label}
        open={open}
        onToggle={() => setOpen((v) => !v)}
        action={action}
      />
      {open && (
        <>
          <div className="flex flex-col gap-1.5 px-2">
            {shown.map((session) => (
              <SectionSessionRow
                key={sessionKey(session)}
                session={session}
                selected={sessionKey(session) === selectedId}
                {...handlers}
              />
            ))}
          </div>
          {sessions.length > limit && (
            <button
              type="button"
              className="mt-1 w-full px-3 py-1.5 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
              onClick={() => setShowAll((v) => !v)}
            >
              {showAll ? "Show less" : `Show ${sessions.length - limit} more`}
            </button>
          )}
        </>
      )}
    </section>
  );
}

interface ProjectDialogState {
  /** undefined → creating; id → renaming */
  readonly id?: string;
  readonly name: string;
}

export function ProjectNameDialog({
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
      <DialogContent>
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

/**
 * Project row actions — shared by the ⋯ dropdown and the right-click context
 * menu. Item is the menu item primitive of whichever menu hosts it.
 */
function ProjectActions({
  project,
  Item,
  onDetails,
  onAdd,
  onRename,
  onDelete,
}: {
  readonly project: Project;
  readonly Item: typeof ContextMenuItem;
  onDetails: (project: Project) => void;
  onAdd: (project: Project) => void;
  onRename: (project: Project) => void;
  onDelete: (project: Project) => void;
}) {
  return (
    <>
      <Item onClick={() => onDetails(project)}>
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        Project details
      </Item>
      <Item onClick={() => onAdd(project)}>
        <HugeiconsIcon icon={UserAdd01Icon} strokeWidth={2} />
        Add session…
      </Item>
      <Item onClick={() => onRename(project)}>
        <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
        Rename project
      </Item>
      <Item variant="destructive" onClick={() => onDelete(project)}>
        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
        Delete project
      </Item>
    </>
  );
}

function ProjectsSection({
  label,
  projects,
  projectsLoading,
  sessions,
  selectedId,
  resolvedCwd,
  ...handlers
}: {
  readonly label: string;
  readonly projects: ReadonlyArray<Project>;
  readonly projectsLoading: boolean;
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  /** Fallback spawn dir for projects with no member sessions yet. */
  readonly resolvedCwd: string;
} & RowHandlers) {
  const [collapsed, setCollapsed] = useUiState<Record<string, boolean>>("ui.collapsedProjects", {});
  const [dialog, setDialog] = useState<ProjectDialogState | null>(null);
  const [deleteFor, setDeleteFor] = useState<Project | null>(null);
  const [detailsFor, setDetailsFor] = useState<Project | null>(null);
  const [addFor, setAddFor] = useState<Project | null>(null);
  const createProject = useCreateProject();
  const renameProject = useRenameProject();
  const deleteProject = useDeleteProject();
  const createSession = useCreateSession();
  const patchSession = usePatchSessionMeta();
  const { data: agents = [] } = useAgents();
  const settings = useStore(settingsStore);
  const focus = useFocus();
  // resolvedCwd belongs to the focused node — a project elsewhere can only
  // fall back to a dir that exists there, so cross-node falls to "/".
  const focusedKey = nodeKey(focusNode(focus));

  // Bucket members per project in one pass instead of filtering `sessions`
  // once per project row on every render. Keys are node-namespaced
  // (`node:id`) — session.projectIds are already namespaced by the merge.
  const membersByProject = useMemo(() => {
    const map = new Map<string, SessionSummary[]>();
    for (const session of sessions) {
      for (const projectId of session.projectIds) {
        const members = map.get(projectId);
        if (members === undefined) map.set(projectId, [session]);
        else members.push(session);
      }
    }
    return map;
  }, [sessions]);

  const newSessionIn = (project: Project, members: ReadonlyArray<SessionSummary>): void => {
    // Spawn in the newest member's cwd — falling back to the owning node's
    // configured default, then its most recent session's dir — on the node
    // that owns the project, then enroll the session via its namespaced
    // project key.
    const cwd =
      members[0]?.cwd ??
      defaultCwdFor(settings, project.node) ??
      recentCwdFor(sessions, project.node) ??
      (nodeKey(project.node) === focusedKey ? resolvedCwd : "/");
    // The focused agent wins when the project sits on the focused node;
    // else the node's configured default — unset → local keeps the roster's
    // first agent, while a peer gets no override and picks its own.
    const focusedAgent =
      focus !== null && nodeKey(project.node) === focusedKey ? focus.agent : null;
    const agent =
      focusedAgent ??
      defaultAgentFor(settings, project.node) ??
      (isLocalNode(project.node) ? agents[0]?.id : undefined);
    void createSession
      .mutateAsync({
        cwd,
        agent,
        node: project.node,
        ...modelArgsFor(agent ?? "", null, settings),
      })
      .then(({ id, agentId }) =>
        patchSession.mutate({
          id,
          agent: agentId ?? agent,
          node: project.node,
          patch: { projectIds: [projectKey(project)] },
        }),
      );
  };

  const submitName = (state: ProjectDialogState, name: string): void => {
    if (state.id === undefined) createProject.mutate({ name });
    else renameProject.mutate({ key: state.id, name });
    setDialog(null);
  };

  const [open, setOpen] = useUiState("ui.section.projects", true);
  return (
    <section className="group/section">
      <SectionHeader
        label={label}
        open={open}
        onToggle={() => setOpen((v) => !v)}
        action={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="New project"
            title="New project"
            onClick={() => setDialog({ name: "" })}
          >
            <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
          </Button>
        }
      />
      {open && !projectsLoading && projects.length === 0 && (
        <p className="px-3 py-1 text-xs text-muted-foreground">No projects yet.</p>
      )}
      {open &&
        projects.map((project) => {
          const members = membersByProject.get(projectKey(project)) ?? [];
          const open = !collapsed[projectKey(project)];
          return (
            <ContextMenu key={projectKey(project)}>
              <ContextMenuTrigger className="group/row relative block">
                <div className="relative px-1.5">
                  <button
                    type="button"
                    className="flex w-full items-center gap-1.5 rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
                    onClick={() =>
                      setCollapsed((prev) => ({
                        ...prev,
                        [projectKey(project)]: !prev[projectKey(project)],
                      }))
                    }
                  >
                    <HugeiconsIcon
                      icon={ChevronRightIcon}
                      strokeWidth={2}
                      className={`size-3.5 shrink-0 text-muted-foreground transition-transform ${open ? "rotate-90" : ""}`}
                    />
                    <HugeiconsIcon
                      icon={FolderLibraryIcon}
                      strokeWidth={2}
                      className="size-4 shrink-0 text-muted-foreground"
                    />
                    <span className="min-w-0 flex-1 truncate font-medium" title={project.name}>
                      {project.name}
                    </span>
                    {/* Indented so the hover-reveal action icons don't cover it. */}
                    {project.node !== undefined && (
                      <span className="mr-12 shrink-0">
                        <NodeBadge node={project.node} />
                      </span>
                    )}
                  </button>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="absolute top-1/2 right-8 -translate-y-1/2 bg-secondary/90 opacity-0 shadow-sm backdrop-blur-sm transition-opacity group-hover/row:opacity-100 hover:bg-secondary focus-visible:opacity-100 data-popup-open:opacity-100"
                    aria-label={`New session in project ${project.name}`}
                    title="New session here"
                    onClick={(event) => {
                      event.stopPropagation();
                      newSessionIn(project, members);
                    }}
                  >
                    <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
                  </Button>
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
                      <ProjectActions
                        project={project}
                        Item={DropdownMenuItem as unknown as typeof ContextMenuItem}
                        onDetails={setDetailsFor}
                        onAdd={setAddFor}
                        onRename={(p) => setDialog({ id: projectKey(p), name: p.name })}
                        onDelete={setDeleteFor}
                      />
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent className="w-44">
                <ProjectActions
                  project={project}
                  Item={ContextMenuItem}
                  onDetails={setDetailsFor}
                  onAdd={setAddFor}
                  onRename={(p) => setDialog({ id: projectKey(p), name: p.name })}
                  onDelete={setDeleteFor}
                />
              </ContextMenuContent>
              {open && (
                <div className="ml-4 flex flex-col gap-1 border-l border-border/50 pl-3">
                  {members.length === 0 && (
                    <div className="flex items-center gap-2 px-2 py-1.5 text-xs text-muted-foreground">
                      Empty.
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => newSessionIn(project, members)}
                      >
                        New session
                      </Button>
                    </div>
                  )}
                  {members.map((session) => (
                    <SectionSessionRow
                      key={sessionKey(session)}
                      session={session}
                      selected={sessionKey(session) === selectedId}
                      {...handlers}
                    />
                  ))}
                </div>
              )}
            </ContextMenu>
          );
        })}
      {dialog !== null && (
        <ProjectNameDialog state={dialog} onClose={() => setDialog(null)} onSubmit={submitName} />
      )}
      {detailsFor !== null && (
        <ProjectDetailsDialog
          project={detailsFor}
          members={membersByProject.get(projectKey(detailsFor)) ?? []}
          onClose={() => setDetailsFor(null)}
          onSelectSession={(id) => {
            handlers.onSelect(id);
            setDetailsFor(null);
          }}
        />
      )}
      {addFor !== null && (
        <AddSessionDialog project={addFor} sessions={sessions} onClose={() => setAddFor(null)} />
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
              onClick={() => deleteFor !== null && deleteProject.mutate(projectKey(deleteFor))}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

/** "Folders" — the cwd tree, kept as its own component for the ui state hook. */
function FoldersSection({
  label,
  sessions,
  selectedId,
  scrollRef,
  hotkeyTarget,
  onNewSession,
  ...handlers
}: {
  readonly label: string;
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  readonly scrollRef: RefObject<HTMLDivElement | null>;
  readonly hotkeyTarget: RefObject<HTMLElement | null>;
  onNewSession: (cwd: string, node?: string) => void;
} & RowHandlers) {
  const [open, setOpen] = useUiState("ui.section.folders", true);
  return (
    <section className="group/section">
      <SectionHeader label={label} open={open} onToggle={() => setOpen((v) => !v)} />
      {open && (
        <SessionTree
          sessions={sessions}
          selectedId={selectedId}
          scrollRef={scrollRef}
          hotkeyTarget={hotkeyTarget}
          onNewSession={onNewSession}
          {...handlers}
        />
      )}
    </section>
  );
}

interface SessionSectionsProps {
  readonly sessions: ReadonlyArray<SessionSummary>;
  readonly recentSessions: ReadonlyArray<SessionSummary>;
  readonly archivedSessions: ReadonlyArray<SessionSummary>;
  readonly selectedId: string | null;
  /** Fallback spawn dir for "New session here" on empty projects. */
  readonly resolvedCwd: string;
  /** false while sessions load or the query failed — gates the tree. */
  readonly showContent: boolean;
  /** Shared sidebar scroller the folders tree virtualizes against. */
  readonly scrollRef: RefObject<HTMLDivElement | null>;
  readonly hotkeyTarget: RefObject<HTMLElement | null>;
  onNewSession: (cwd: string, node?: string) => void;
  onSelect: (id: string) => void;
  onDetails: (id: string, rename: boolean) => void;
  onDelete: (session: SessionSummary) => void;
}

/**
 * The sidebar's session sections, rendered in the order/visibility/labels/
 * limits from `settings.sidebar.sections`. Disabled sections skip render;
 * their data still computes so re-enabling is instant.
 */
export function SessionSections({
  sessions,
  recentSessions,
  archivedSessions,
  selectedId,
  resolvedCwd,
  showContent,
  scrollRef,
  hotkeyTarget,
  onNewSession,
  ...handlers
}: SessionSectionsProps) {
  const { data: projects, isLoading: projectsLoading } = useProjects();
  const { data: agents = [] } = useAgents();
  const createSession = useCreateSession();
  const settings = useStore(settingsStore);
  const focus = useFocus();
  // Membership comes from a live-query view the engine maintains
  // incrementally — it only re-derives when a session's `pinned` actually
  // flips. The `sessions` prop still supplies ordering + UI filters.
  const { data: pinnedRows } = useLiveQuery((q) =>
    q
      .from({ s: sessionsCollection })
      .where(({ s }) => eq(s.pinned, true))
      .select(({ s }) => ({ id: s.id, agent: s.agent, node: s.node })),
  );
  const pinned = useMemo(() => {
    const keys = new Set(pinnedRows.map(sessionKey));
    return sessions.filter((session) => keys.has(sessionKey(session)));
  }, [sessions, pinnedRows]);

  // Sections that would render nothing (disabled, or an empty flat list)
  // drop out so the block collapses entirely — as before. The Projects
  // header only shows when it or a sibling flat section has content.
  const hasTopContent = pinned.length > 0 || recentSessions.length > 0 || projects.length > 0;
  const visible = settings.sidebar.sections.filter((section) => {
    if (!section.enabled) return false;
    switch (section.id) {
      case "pinned":
        return pinned.length > 0;
      case "projects":
        return hasTopContent;
      case "sessions":
        return recentSessions.length > 0;
      case "folders":
        return showContent;
      case "archived":
        return archivedSessions.length > 0;
    }
  });
  if (visible.length === 0) return null;
  // The separator line originally only topped the pinned/projects/recents
  // block — folders/archived sat below it, line-free.
  const hasTopSection = visible.some(
    (section) => section.id === "pinned" || section.id === "projects" || section.id === "sessions",
  );
  return (
    <div className={cn("shrink-0 pb-2", hasTopSection && "border-t border-border")}>
      {visible.map((section) => {
        const label = sidebarSectionLabel(section);
        switch (section.id) {
          case "pinned":
            return (
              <FlatSection
                key={section.id}
                label={label}
                sectionKey="pinned"
                limit={sidebarSectionLimit(section) ?? 5}
                sessions={pinned}
                selectedId={selectedId}
                {...handlers}
              />
            );
          case "projects":
            return (
              <ProjectsSection
                key={section.id}
                label={label}
                projects={projects}
                projectsLoading={projectsLoading}
                sessions={sessions}
                selectedId={selectedId}
                resolvedCwd={resolvedCwd}
                {...handlers}
              />
            );
          case "sessions":
            return (
              <FlatSection
                key={section.id}
                label={label}
                sectionKey="recents"
                limit={sidebarSectionLimit(section) ?? 8}
                sessions={recentSessions}
                selectedId={selectedId}
                action={
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label="New session"
                    title="New session"
                    onClick={() => {
                      // The recents "+" targets the focus, same as the main
                      // button — resolvedCwd is already resolved for it.
                      const target = resolveCreateTarget(focus, settings, undefined);
                      const agent =
                        target.agent ?? (isLocalNode(target.node) ? agents[0]?.id : undefined);
                      createSession.mutate({
                        cwd: resolvedCwd,
                        agent,
                        node: target.node,
                        ...modelArgsFor(agent ?? "", null, settings),
                      });
                    }}
                  >
                    <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
                  </Button>
                }
                {...handlers}
              />
            );
          case "folders":
            return (
              <FoldersSection
                key={section.id}
                label={label}
                sessions={sessions}
                selectedId={selectedId}
                scrollRef={scrollRef}
                hotkeyTarget={hotkeyTarget}
                onNewSession={onNewSession}
                {...handlers}
              />
            );
          case "archived":
            return (
              <FlatSection
                key={section.id}
                label={label}
                sectionKey="archived"
                limit={sidebarSectionLimit(section) ?? 20}
                sessions={archivedSessions}
                selectedId={selectedId}
                {...handlers}
              />
            );
        }
      })}
    </div>
  );
}

/** Project info — name + member list. */
function ProjectDetailsDialog({
  project,
  members,
  onClose,
  onSelectSession,
}: {
  readonly project: Project;
  readonly members: ReadonlyArray<SessionSummary>;
  onClose: () => void;
  onSelectSession: (id: string) => void;
}) {
  // Projects are node-local — show which machine owns this one (peer rows
  // also get the origin so the label isn't the only identifier).
  const { peers } = useNodes();
  const nodeLabel = useNodeLabel(project.node);
  const peer = peers.find((p) => p.id === project.node);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{project.name}</DialogTitle>
          <DialogDescription>
            {members.length} session{members.length === 1 ? "" : "s"} · on {nodeLabel}
            {peer !== undefined && ` (${new URL(peer.url).host})`}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-1.5 px-2">
          {members.length === 0 && (
            <p className="text-sm text-muted-foreground">No sessions in this project.</p>
          )}
          {members.map((session) => (
            <button
              key={sessionKey(session)}
              type="button"
              className="rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
              onClick={() => onSelectSession(sessionKey(session))}
            >
              <span className="block truncate font-medium">{session.title}</span>
              <span className="block truncate text-xs text-muted-foreground">
                {session.agent} · {formatUpdated(session.updatedAt)}
              </span>
            </button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Assign an existing session to the project — searchable picker. */
function AddSessionDialog({
  project,
  sessions,
  onClose,
}: {
  readonly project: Project;
  readonly sessions: ReadonlyArray<SessionSummary>;
  onClose: () => void;
}) {
  const patch = usePatchSessionMeta();
  const [filter, setFilter] = useState("");
  // Projects are node-local — only sessions on the same node can join.
  const candidates = sessions.filter(
    (s) =>
      nodeKey(s.node) === nodeKey(project.node) &&
      !s.projectIds.includes(projectKey(project)) &&
      s.title.toLowerCase().includes(filter.trim().toLowerCase()),
  );
  const add = (session: SessionSummary): void =>
    patch.mutate(
      {
        id: session.id,
        agent: session.agent,
        node: session.node,
        patch: { projectIds: [...session.projectIds, projectKey(project)] },
      },
      { onSuccess: onClose },
    );
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add session to {project.name}</DialogTitle>
          <DialogDescription>Pick a session to include in this project.</DialogDescription>
        </DialogHeader>
        <Input
          placeholder="Filter sessions…"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          autoFocus
        />
        <ScrollArea className="max-h-64">
          <div className="flex flex-col gap-1.5 px-2">
            {candidates.length === 0 && (
              <p className="px-1 py-2 text-sm text-muted-foreground">
                {filter.trim() === "" ? "All sessions are already in this project." : "No matches."}
              </p>
            )}
            {candidates.map((session) => (
              <button
                key={sessionKey(session)}
                type="button"
                className="rounded-md px-2 py-2 text-left text-sm transition-colors hover:bg-accent/60"
                onClick={() => add(session)}
              >
                <span className="block truncate font-medium">{session.title}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  {session.agent} · {session.cwd}
                </span>
              </button>
            ))}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}
