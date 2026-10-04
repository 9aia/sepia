import { useEffect, useMemo, useRef, useState } from "react";
import {
  CheckIcon,
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
import { Badge } from "./ui/badge";
import { Spinner } from "./ui/spinner";
import { Marker, MarkerContent, MarkerIcon } from "./marker";
import { MessageResponse } from "./streamdown";
import { formatDuration } from "../lib/format";
import type { ToolCallStatus, ToolFileDiff, ToolLocation } from "../lib/types";
import {
  diffFence,
  fileDiffView,
  toolSummary,
  type ToolCategory,
  type ToolSegment,
} from "../lib/toolDisplay";

const CATEGORY_ICON: Record<ToolCategory, typeof WrenchIcon> = {
  exec: ComputerTerminal01Icon,
  edit: FileEditIcon,
  read: ViewIcon,
  search: Search01Icon,
  fetch: GlobeIcon,
  todo: Task01Icon,
  other: WrenchIcon,
};

/**
 * Diff text rendered through the shared markdown pipeline as a `diff`
 * code block — streamdown/Shiki colors the +/- lines and the CodeBlock
 * chrome adds the lang label + copy button. `max-h` keeps a big recorded
 * diff bounded like the other segments.
 */
function DiffBlock({ text }: { readonly text: string }) {
  return (
    <MessageResponse className="[&_pre]:max-h-72 [&_pre]:overflow-y-auto">
      {diffFence(text)}
    </MessageResponse>
  );
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
      return <DiffBlock text={segment.text} />;
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
  status,
  exitCode,
  durationMs,
  diffs,
  locations,
}: {
  readonly toolName: string;
  readonly done: boolean;
  /**
   * The call's args as JSON text — a live row's accumulated `args` stream or
   * a history row's JSON-encoded `ToolCall.arguments`; undefined on rows
   * from stores that recorded no args.
   */
  readonly args?: string;
  readonly content: string;
  /** IR v2 outcome — absent on rows from older stores, where `done` still rules. */
  readonly status?: ToolCallStatus;
  /** Authoritative exit code; overrides any parsed from `content`. */
  readonly exitCode?: number;
  readonly durationMs?: number;
  /** Recorded before/after payloads — the "files changed" marker + diff view. */
  readonly diffs?: ReadonlyArray<ToolFileDiff>;
  /** Files the call touched when no diff was recorded (refs only). */
  readonly locations?: ReadonlyArray<ToolLocation>;
}) {
  const display = toolSummary(toolName, args, content, { exitCode });
  const fileViews = useMemo(() => (diffs ?? []).map(fileDiffView), [diffs]);
  const running = status === "pending" || !done;
  // The marker detail is `$ cmd`-style only when a command was actually
  // extracted — a shell-id fallback (get_output) stays plain.
  const hasCommand = display.segments.some((s) => s.kind === "command");
  // Refs stand in only when no diff was recorded — and only on calls whose
  // category can change files; a read's location is already the detail line.
  const locationOnly =
    fileViews.length === 0 && display.category === "edit" ? (locations ?? []) : [];
  const duration = durationMs === undefined ? "" : formatDuration(durationMs);
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
        <Marker role={running ? "status" : undefined}>
          <MarkerIcon>
            <HugeiconsIcon icon={CATEGORY_ICON[display.category]} strokeWidth={2} />
          </MarkerIcon>
          <MarkerContent className="flex items-center gap-2">
            <span className="shrink-0 font-medium text-foreground/80">{display.label}</span>
            {display.detail !== undefined && (
              <code className="min-w-0 truncate text-muted-foreground">
                {hasCommand ? `$ ${display.detail}` : display.detail}
              </code>
            )}
            {status === "error" ? (
              <Badge variant="destructive" className="h-4 px-1.5 text-[10px]">
                Error
              </Badge>
            ) : status === "success" ? (
              <HugeiconsIcon
                icon={CheckIcon}
                strokeWidth={2}
                className="size-3.5 shrink-0 text-muted-foreground"
              />
            ) : running ? (
              <Badge variant="secondary" className="h-4 gap-1 px-1.5 text-[10px]">
                <Spinner aria-hidden="true" className="size-2.5" />
                Running
              </Badge>
            ) : (
              <span className="shrink-0 text-muted-foreground">Completed</span>
            )}
            {duration !== "" && (
              <span className="shrink-0 text-muted-foreground/70">({duration})</span>
            )}
            {fileViews.length > 0 && (
              <span className="shrink-0 rounded-sm bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                {fileViews.length} file{fileViews.length === 1 ? "" : "s"} changed
              </span>
            )}
            {fileViews.length === 0 && locationOnly.length > 0 && (
              <span className="shrink-0 rounded-sm bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                {locationOnly.length} file{locationOnly.length === 1 ? "" : "s"}
              </span>
            )}
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
          {display.segments.length === 0 && fileViews.length === 0 && locationOnly.length === 0 ? (
            <span className="text-muted-foreground/70 italic">
              {done ? "No output" : "Waiting for input…"}
            </span>
          ) : (
            <>
              {display.segments.map((segment, i) => (
                <ToolSegmentView key={i} segment={segment} />
              ))}
              {fileViews.map((file) => (
                <div key={file.path} className="flex flex-col gap-1">
                  <div className="flex min-w-0 items-baseline gap-2">
                    <code className="min-w-0 truncate text-foreground/70" title={file.path}>
                      {file.path}
                    </code>
                    <span className="shrink-0 text-[10px] whitespace-nowrap">
                      <span className="text-emerald-600 dark:text-emerald-400">+{file.added}</span>{" "}
                      <span className="text-red-600 dark:text-red-400">−{file.removed}</span>
                    </span>
                  </div>
                  {file.text !== "" && <DiffBlock text={file.text} />}
                </div>
              ))}
              {locationOnly.map((loc) => (
                <code key={`${loc.path}:${loc.line ?? ""}`} className="truncate" title={loc.path}>
                  {loc.path}
                  {loc.line === undefined ? "" : `:${loc.line}`}
                </code>
              ))}
            </>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}
