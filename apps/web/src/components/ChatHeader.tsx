import { useState } from "react";
import { MoreVerticalIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../lib/types";
import { setDetailsFor, setSelectedId } from "../lib/store";
import { sessionKey } from "../lib/format";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./ui/alert-dialog";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import type {
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "./ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { SidebarTrigger } from "./ui/sidebar";
import { SessionActions } from "./session-list/SessionActions";

interface ChatHeaderProps {
  readonly session: SessionSummary;
  readonly running: boolean;
}

export function ChatHeader({ session, running }: ChatHeaderProps) {
  const deleteMutation = useDeleteSession();
  const [confirmOpen, setConfirmOpen] = useState(false);
  return (
    <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
      <SidebarTrigger />
      <button
        type="button"
        className="min-w-0 flex-1 cursor-pointer rounded-md px-1 py-0.5 text-left transition-colors hover:bg-accent/60 focus-visible:ring-1 focus-visible:ring-offset-4 focus-visible:ring-offset-background focus-visible:ring-ring"
        aria-label={`Session details: ${session.title}`}
        onClick={() => setDetailsFor({ id: sessionKey(session), rename: false })}
      >
        <h2 className="m-0 truncate text-[15px]">{session.title}</h2>
        <span className="block truncate text-xs text-muted-foreground" title={session.cwd}>
          {session.cwd}
        </span>
      </button>
      <div className="flex shrink-0 items-center gap-2">
        {(session.busy || running) && <Badge variant="destructive">busy</Badge>}

        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Session actions"
                title="Session actions"
              />
            }
          >
            <HugeiconsIcon icon={MoreVerticalIcon} strokeWidth={2} />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-52">
            <SessionActions
              hideOpen
              Item={DropdownMenuItem as unknown as typeof ContextMenuItem}
              Separator={DropdownMenuSeparator}
              Sub={DropdownMenuSub as unknown as typeof ContextMenuSub}
              SubTrigger={DropdownMenuSubTrigger as unknown as typeof ContextMenuSubTrigger}
              SubContent={DropdownMenuSubContent as unknown as typeof ContextMenuSubContent}
              session={session}
              onSelect={setSelectedId}
              onDetails={(id, rename) => setDetailsFor({ id, rename })}
              onRequestDelete={() => setConfirmOpen(true)}
            />
          </DropdownMenuContent>
        </DropdownMenu>
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
              <AlertDialogAction
                variant="destructive"
                onClick={() => deleteMutation.mutate({ id: session.id, agent: session.agent })}
              >
                Delete
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </header>
  );
}
