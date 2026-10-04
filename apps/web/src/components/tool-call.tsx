import { useEffect, useRef, useState } from "react";
import {
  ChevronDownIcon,
  ComputerTerminal01Icon,
  FileEditIcon,
  GlobeIcon,
  Search01Icon,
  Task01Icon,
  ViewIcon,
  WrenchIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { cn } from "@/lib/utils";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { MessageResponse } from "./streamdown";
import { toolSummary, type ToolCategory, type ToolSegment } from "../lib/toolDisplay";

const CATEGORY_ICON: Record<ToolCategory, typeof WrenchIcon> = {
  exec: ComputerTerminal01Icon,
  edit: FileEditIcon,
  read: ViewIcon,
  search: Search01Icon,
  fetch: GlobeIcon,
  todo: Task01Icon,
  other: WrenchIcon,
};

function DiffLine({ line }: { readonly line: string }) {
  const cls = line.startsWith("+")
    ? "text-emerald-600 dark:text-emerald-400"
    : line.startsWith("-")
      ? "text-red-600 dark:text-red-400"
      : "";
  return <div className={cls}>{line === "" ? " " : line}</div>;
}

function ToolSegmentView({ segment }: { readonly segment: ToolSegment }) {
  switch (segment.kind) {
    case "command":
      return (
        <div className="flex items-start gap-1.5 font-mono text-[11px]">
          <span className="shrink-0 text-muted-foreground/70 select-none">$</span>
          <span className="min-w-0 break-words whitespace-pre-wrap text-foreground/80">
            {segment.text}
          </span>
        </div>
      );
    case "code":
      return (
        <pre className="max-h-72 overflow-auto rounded-md bg-muted/50 p-2.5 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-words">
          {segment.text}
        </pre>
      );
    case "diff":
      return (
        <pre className="max-h-72 overflow-auto rounded-md bg-muted/50 p-2.5 font-mono text-[11px] leading-relaxed">
          {segment.text.split("\n").map((line, i) => (
            <DiffLine key={i} line={line} />
          ))}
        </pre>
      );
    case "note":
      return (
        <div
          className={cn(
            "text-[11px]",
            segment.error === true ? "text-destructive" : "text-muted-foreground/80",
          )}
        >
          {segment.text}
        </div>
      );
    case "markdown":
      return <MessageResponse>{segment.text}</MessageResponse>;
  }
}

/**
 * A tool call — the collapsed state is a Marker line (icon + label + the
 * salient argument + status); expanding shows structured args/result
 * segments parsed by `toolSummary`. Streams open while running, then
 * collapses shortly after completion (same cadence as ReasoningBlock).
 */
export function ToolCall({
  toolName,
  done,
  args,
  content,
}: {
  readonly toolName: string;
  readonly done: boolean;
  /** Live arg stream (JSON); history rows pass undefined. */
  readonly args?: string;
  readonly content: string;
}) {
  const display = toolSummary(toolName, args, content);
  const [open, setOpen] = useState(!done);
  const everStreamed = useRef(!done);

  useEffect(() => {
    if (!done || !everStreamed.current) return;
    const timer = setTimeout(() => setOpen(false), 1500);
    return () => clearTimeout(timer);
  }, [done]);

  return (
    <Collapsible className="group/tool-call" open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="block w-full rounded-md transition-colors hover:bg-accent/50">
        <Marker>
          <MarkerIcon>
            <HugeiconsIcon icon={CATEGORY_ICON[display.category]} strokeWidth={2} />
          </MarkerIcon>
          <MarkerContent className="flex items-center gap-2">
            <span className="shrink-0 font-medium text-foreground/80">{display.label}</span>
            {display.detail !== undefined && (
              <code className="min-w-0 truncate text-muted-foreground">{display.detail}</code>
            )}
            <span
              className={cn(
                "shrink-0",
                done ? "text-muted-foreground" : "animate-pulse text-primary",
              )}
            >
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
        <div className="ml-6 flex flex-col gap-1.5 border-l-2 border-border/50 py-1 pl-3 text-xs text-muted-foreground">
          {display.segments.length === 0 ? (
            <span className="text-muted-foreground/70 italic">
              {done ? "No output" : "Waiting for input…"}
            </span>
          ) : (
            display.segments.map((segment, i) => <ToolSegmentView key={i} segment={segment} />)
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
