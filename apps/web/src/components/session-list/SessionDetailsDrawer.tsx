import { useEffect, useRef, useState } from "react";
import type { SessionSummary } from "../../lib/types";
import { useRenameSession } from "../../hooks/query/useRenameSession";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
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
  children,
}: {
  readonly label: string;
  readonly children: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-4 py-2 text-sm">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 text-right break-all">{children}</span>
    </div>
  );
}

interface SessionDetailsDrawerProps {
  readonly session: SessionSummary | undefined;
  /** Focus the title field on open (context-menu "Rename…"). */
  readonly focusRename: boolean;
  onClose: () => void;
}

export function SessionDetailsDrawer({ session, focusRename, onClose }: SessionDetailsDrawerProps) {
  const renameMutation = useRenameSession();
  const [title, setTitle] = useState(session?.title ?? "");
  const titleRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    setTitle(session?.title ?? "");
    if (session !== undefined && focusRename) {
      requestAnimationFrame(() => {
        titleRef.current?.focus();
        titleRef.current?.select();
      });
    }
  }, [session, focusRename]);

  const dirty = title.trim() !== "" && title.trim() !== session?.title;
  const rename = (): void => {
    if (session === undefined || !dirty) return;
    renameMutation.mutate({ id: session.id, title: title.trim() });
  };

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
              <div className="flex items-center gap-1.5">
                <Input
                  ref={titleRef}
                  value={title}
                  aria-label="Session title"
                  onChange={(event) => setTitle(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") rename();
                  }}
                />
                <Button size="sm" disabled={!dirty || renameMutation.isPending} onClick={rename}>
                  Rename
                </Button>
              </div>
              {renameMutation.isError && (
                <p className="text-xs text-destructive">
                  {renameMutation.error instanceof Error
                    ? renameMutation.error.message
                    : "Failed to rename session"}
                </p>
              )}

              <div className="mt-2 divide-y divide-border/50">
                <Detail label="ID">
                  <span className="font-mono text-xs">{session.id}</span>
                </Detail>
                <Detail label="Working directory">
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

            <DrawerFooter>
              <DrawerClose render={<Button variant="outline" />}>Close</DrawerClose>
            </DrawerFooter>
          </>
        )}
      </DrawerContent>
    </Drawer>
  );
}
