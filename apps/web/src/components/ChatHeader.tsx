import type { SessionSummary } from "../lib/types";
import type { StreamStatus } from "../lib/api";
import { setDetailsFor } from "../lib/store";
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
      <button
        type="button"
        className="min-w-0 flex-1 cursor-pointer rounded-md px-1 py-0.5 text-left transition-colors hover:bg-accent/60 focus-visible:ring-1 focus-visible:ring-ring"
        aria-label={`Session details: ${session.title}`}
        onClick={() => setDetailsFor({ id: session.id, rename: false })}
      >
        <h2 className="m-0 truncate text-[15px]">{session.title}</h2>
        <span className="block truncate text-xs text-muted-foreground" title={session.cwd}>
          {session.cwd}
        </span>
      </button>
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
