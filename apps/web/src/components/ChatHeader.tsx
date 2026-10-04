import type { SessionSummary } from "../lib/types";
import type { StreamStatus } from "../lib/api";
import { Badge } from "./ui/badge";
import { SidebarTrigger } from "./ui/sidebar";

interface ChatHeaderProps {
  readonly session: SessionSummary;
  readonly readOnly: boolean;
  readonly running: boolean;
  readonly streamStatus: StreamStatus;
}

export function ChatHeader({ session, readOnly, running, streamStatus }: ChatHeaderProps) {
  return (
    <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
      <SidebarTrigger />
      <div className="min-w-0 flex-1">
        <h2 className="m-0 truncate text-[15px]">{session.title}</h2>
        <span className="block truncate text-xs text-muted-foreground" title={session.cwd}>
          {session.cwd}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {readOnly && <Badge variant="secondary">read-only</Badge>}
        {(session.busy || running) && <Badge variant="destructive">busy</Badge>}
        {streamStatus === "reconnecting" && (
          <Badge variant="outline" role="status">
            reconnecting…
          </Badge>
        )}
      </div>
    </header>
  );
}
