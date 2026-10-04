import { useEffect, useRef, useState } from "react";
import {
  Copy01Icon,
  Delete02Icon,
  Edit02Icon,
  FolderOpenIcon,
  Tick02Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { SessionSummary } from "../../lib/types";
import { useRenameSession } from "../../hooks/query/useRenameSession";
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
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
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
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerTitle,
} from "../ui/drawer";
import { Input } from "../ui/input";

function Detail({
  label,
  copyValue,
  children,
}: {
  readonly label: string;
  readonly copyValue?: string;
  readonly children: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  const copy = (): void => {
    if (copyValue === undefined) return;
    void navigator.clipboard.writeText(copyValue).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      },
      () => undefined,
    );
  };
  return (
    <div className="flex items-start justify-between gap-4 py-2 text-sm">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="flex min-w-0 items-center justify-end gap-1 text-right break-all">
        {children}
        {copyValue !== undefined && (
          <Button
            variant="ghost"
            size="icon-xs"
            className="size-5 shrink-0 text-muted-foreground"
            aria-label={`Copy ${label}`}
            title={`Copy ${label}`}
            onClick={copy}
          >
            <HugeiconsIcon icon={copied ? Tick02Icon : Copy01Icon} strokeWidth={2} />
          </Button>
        )}
      </span>
    </div>
  );
}

function RenameDialog({
  session,
  open,
  onOpenChange,
}: {
  readonly session: SessionSummary;
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const renameMutation = useRenameSession();
  const [title, setTitle] = useState(session.title);
  const titleRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setTitle(session.title);
    requestAnimationFrame(() => {
      titleRef.current?.focus();
      titleRef.current?.select();
    });
  }, [open, session.title]);

  const dirty = title.trim() !== "" && title.trim() !== session.title;
  const rename = (): void => {
    if (!dirty) return;
    renameMutation.mutate(
      { id: session.id, title: title.trim() },
      { onSuccess: () => onOpenChange(false) },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename session</DialogTitle>
          <DialogDescription>
            Saved as a Sepia overlay — the agent&apos;s own store is read-only.
          </DialogDescription>
        </DialogHeader>
        <Input
          ref={titleRef}
          value={title}
          aria-label="Session title"
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") rename();
          }}
        />
        {renameMutation.isError && (
          <p className="text-xs text-destructive">
            {renameMutation.error instanceof Error
              ? renameMutation.error.message
              : "Failed to rename session"}
          </p>
        )}
        <DialogFooter>
          <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
          <Button disabled={!dirty || renameMutation.isPending} onClick={rename}>
            Rename
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface SessionDetailsDrawerProps {
  readonly session: SessionSummary | undefined;
  /** Focus the title field on open (context-menu "Rename…"). */
  readonly focusRename: boolean;
  onClose: () => void;
  onOpen: (id: string) => void;
  onDelete: (id: string) => void;
}

export function SessionDetailsDrawer({
  session,
  focusRename,
  onClose,
  onOpen,
  onDelete,
}: SessionDetailsDrawerProps) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);

  useEffect(() => {
    if (session !== undefined && focusRename) setRenameOpen(true);
  }, [session, focusRename]);

  return (
    <Drawer
      swipeDirection="right"
      open={session !== undefined}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DrawerContent className="[--drawer-content-height:100dvh] [--drawer-content-width:22rem]">
        {session !== undefined && (
          <>
            <DrawerHeader>
              <DrawerTitle>Session details</DrawerTitle>
              <DrawerDescription>
                Rename lives here as an overlay — the underlying agent store is read-only.
              </DrawerDescription>
            </DrawerHeader>

            <div className="flex flex-col gap-1 overflow-y-auto px-4 py-3">
              <div className="mt-2 divide-y divide-border/50">
                <Detail label="ID" copyValue={session.id}>
                  <span className="font-mono text-xs">{session.id}</span>
                </Detail>
                <Detail label="Working directory" copyValue={session.cwd}>
                  <span className="font-mono text-xs" title={session.cwd}>
                    {session.cwd}
                  </span>
                </Detail>
                <Detail label="Agent">
                  <Badge variant="secondary">{session.agent}</Badge>
                </Detail>
                <Detail label="Source">{session.source}</Detail>
                <Detail label="Last active">{new Date(session.updatedAt).toLocaleString()}</Detail>
                <Detail label="Status">
                  {session.locked ? (
                    <Badge
                      variant="destructive"
                      title={`Locked by pid ${session.lockHolderPid ?? "unknown"}`}
                    >
                      locked
                    </Badge>
                  ) : session.busy ? (
                    <Badge variant="destructive">busy</Badge>
                  ) : (
                    <Badge variant="secondary">free</Badge>
                  )}
                </Detail>
              </div>
            </div>

            <DrawerFooter className="flex-row justify-end gap-2">
              <DrawerClose render={<Button variant="outline" />}>Close</DrawerClose>
              <Button variant="destructive" onClick={() => setConfirmOpen(true)}>
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                Delete
              </Button>
              <Button variant="outline" onClick={() => setRenameOpen(true)}>
                <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
                Rename
              </Button>
              <Button
                onClick={() => {
                  onOpen(session.id);
                  onClose();
                }}
              >
                <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
                Open session
              </Button>
            </DrawerFooter>
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
                    onClick={() => {
                      onDelete(session.id);
                      setConfirmOpen(false);
                      onClose();
                    }}
                  >
                    Delete
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
            <RenameDialog session={session} open={renameOpen} onOpenChange={setRenameOpen} />
          </>
        )}
      </DrawerContent>
    </Drawer>
  );
}
