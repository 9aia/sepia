import { useEffect, useRef, useState } from "react";
import { BrainIcon, ChevronDownIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
import { Badge } from "./ui/badge";
import { Spinner } from "./ui/spinner";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { MessageResponse } from "./streamdown";

/**
 * A reasoning/thinking block — the collapsed state is a Marker line (icon +
 * label + chevron), expanding shows the reasoning text. Streams open, then
 * collapses shortly after the run finishes.
 */
export function ReasoningBlock({
  done,
  content,
}: {
  readonly done: boolean;
  readonly content: string;
}) {
  const [open, setOpen] = useState(!done);
  const everStreamed = useRef(!done);

  useEffect(() => {
    if (!done || !everStreamed.current) return;
    const timer = setTimeout(() => setOpen(false), 1000);
    return () => clearTimeout(timer);
  }, [done]);

  return (
    <Collapsible className="group/reasoning" open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="block w-full rounded-md transition-colors hover:bg-accent/50">
        <Marker role={!done ? "status" : undefined}>
          <MarkerIcon>
            <HugeiconsIcon icon={BrainIcon} strokeWidth={2} />
          </MarkerIcon>
          <MarkerContent className="flex items-center gap-2">
            <span className="min-w-0 truncate font-medium text-foreground/80">Reasoning</span>
            {!done && (
              <Badge variant="secondary" className="h-4 gap-1 px-1.5 text-[10px]">
                <Spinner aria-hidden="true" className="size-2.5" />
                Thinking
              </Badge>
            )}
            <HugeiconsIcon
              icon={ChevronDownIcon}
              strokeWidth={2}
              className="ml-auto size-3.5 shrink-0 transition-transform group-data-open/reasoning:rotate-180"
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
