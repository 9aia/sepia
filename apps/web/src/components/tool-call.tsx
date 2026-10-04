import { ChevronDownIcon, WrenchIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { MessageResponse } from "./streamdown";

/**
 * A tool call — the collapsed state is a Marker line (icon + name + status),
 * expanding shows the args/result.
 */
export function ToolCall({
  toolName,
  done,
  content,
}: {
  readonly toolName: string;
  readonly done: boolean;
  readonly content: string;
}) {
  return (
    <Collapsible className="group/tool-call">
      <CollapsibleTrigger className="block w-full rounded-md transition-colors hover:bg-accent/50">
        <Marker>
          <MarkerIcon>
            <HugeiconsIcon icon={WrenchIcon} strokeWidth={2} />
          </MarkerIcon>
          <MarkerContent className="flex items-center gap-2">
            <span className="min-w-0 truncate font-medium text-foreground/80">{toolName}</span>
            <span className={done ? "text-muted-foreground" : "animate-pulse text-primary"}>
              {done ? "Completed" : "Running…"}
            </span>
            <HugeiconsIcon
              icon={ChevronDownIcon}
              strokeWidth={2}
              className="ml-auto size-3.5 shrink-0 transition-transform group-data-open/tool-call:rotate-180"
            />
          </MarkerContent>
        </Marker>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="ml-6 border-l-2 border-border/50 py-1 pl-3 text-xs text-muted-foreground">
          <MessageResponse>{content}</MessageResponse>
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
