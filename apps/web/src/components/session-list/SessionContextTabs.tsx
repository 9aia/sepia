import { useEffect, useMemo } from "react";
import { ChevronDownIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { flattenHistory, useHistory } from "../../hooks/query/useHistory";
import { parseSystemContext } from "../../lib/systemContext";
import type { SessionSummary } from "../../lib/types";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { Spinner } from "../ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../ui/tabs";
import { MessageResponse } from "../streamdown";

/*
 * The parser's return shape is deliberately loose here: a concurrent change
 * may enrich rules/reports (content bodies, titles) and add `skills` /
 * `promptSections`. Every field is normalized out of `unknown` so the tabs
 * work with whatever the current `SystemContext` carries — absent fields
 * just hide their tab.
 */

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

/** First non-empty string field, trimmed. */
const pick = (value: unknown, ...keys: ReadonlyArray<string>): string | undefined => {
  if (!isRecord(value)) return undefined;
  for (const key of keys) {
    const field = value[key];
    if (typeof field === "string" && field.trim() !== "") return field.trim();
  }
  return undefined;
};

const basename = (path: string): string =>
  path
    .split("/")
    .filter((part) => part !== "")
    .pop() ?? path;

/** First non-empty line, markdown heading marks stripped — a card title. */
const firstLine = (body: string): string | undefined => {
  const line = body
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l !== "");
  return line?.replace(/^#+\s*/, "").trim() || undefined;
};

/** A named context file — a rule or a skill entry. */
interface ContextEntry {
  title: string;
  path?: string;
  description?: string;
  content?: string;
}

const toEntry = (value: unknown, index: number): ContextEntry | null => {
  if (typeof value === "string") {
    const title = value.trim();
    return title === "" ? null : { title };
  }
  if (!isRecord(value)) return null;
  const path = pick(value, "path", "file", "uri");
  const title =
    pick(value, "name", "title", "label") ?? (path ? basename(path) : `Entry ${index + 1}`);
  return {
    title,
    ...(path !== undefined ? { path } : {}),
    ...(pick(value, "description", "summary") !== undefined
      ? { description: pick(value, "description", "summary") }
      : {}),
    ...(pick(value, "content", "body", "text", "markdown") !== undefined
      ? { content: pick(value, "content", "body", "text", "markdown") }
      : {}),
  };
};

/** A sub-agent completion report — markdown body under a short title. */
interface Report {
  title: string;
  body: string;
}

const toReport = (value: unknown, index: number): Report | null => {
  const fallback = `Report ${index + 1}`;
  if (typeof value === "string") {
    const body = value.trim();
    return body === "" ? null : { title: firstLine(body) ?? fallback, body };
  }
  if (!isRecord(value)) return null;
  const body = pick(value, "body", "content", "text", "report", "markdown");
  if (body === undefined) return null;
  return { title: pick(value, "title", "name", "label") ?? firstLine(body) ?? fallback, body };
};

/** One labeled slice of the system prompt. */
interface PromptSection {
  title: string;
  body: string;
}

const toSection = (value: unknown, index: number): PromptSection | null => {
  if (typeof value === "string") {
    const body = value.trim();
    return body === "" ? null : { title: `Section ${index + 1}`, body };
  }
  if (!isRecord(value)) return null;
  const body = pick(value, "body", "text", "content", "markdown");
  if (body === undefined) return null;
  return {
    title: pick(value, "title", "heading", "name", "label") ?? `Section ${index + 1}`,
    body,
  };
};

const toList = <T,>(raw: unknown, map: (value: unknown, index: number) => T | null): T[] =>
  Array.isArray(raw)
    ? raw.flatMap((v, i) => {
        const mapped = map(v, i);
        return mapped === null ? [] : [mapped];
      })
    : [];

const Chevron = () => (
  <HugeiconsIcon
    icon={ChevronDownIcon}
    strokeWidth={2}
    className="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[panel-open]:rotate-180"
  />
);

/** Title row shared by the collapsible cards. */
function EntryHeading({ entry }: { readonly entry: ContextEntry }) {
  return (
    <>
      <span className="min-w-0 flex-1 truncate font-medium text-foreground/80" title={entry.title}>
        {entry.title}
      </span>
      {entry.path !== undefined && (
        <code
          className="max-w-2/5 shrink-0 truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
          title={entry.path}
        >
          {entry.path}
        </code>
      )}
    </>
  );
}

/** Rule/skill card — a bare row when there's nothing to expand into. */
function EntryCard({ entry }: { readonly entry: ContextEntry }) {
  if (entry.content === undefined && entry.description === undefined) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-2.5 py-1.5 text-xs">
        <EntryHeading entry={entry} />
      </div>
    );
  }
  return (
    <Collapsible className="rounded-md border border-border/60 bg-muted/30">
      <CollapsibleTrigger className="group flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-accent/40">
        <EntryHeading entry={entry} />
        <Chevron />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="max-h-64 overflow-y-auto border-t border-border/60 px-2.5 py-2 text-xs">
          {entry.description !== undefined && (
            <p className="text-muted-foreground">{entry.description}</p>
          )}
          {entry.content !== undefined && (
            <MessageResponse className="text-foreground/80">{entry.content}</MessageResponse>
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function ReportCard({ report }: { readonly report: Report }) {
  return (
    <Collapsible className="rounded-md border border-border/60 bg-muted/30">
      <CollapsibleTrigger className="group flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-accent/40">
        <span
          className="min-w-0 flex-1 truncate font-medium text-foreground/80"
          title={report.title}
        >
          {report.title}
        </span>
        <Chevron />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="max-h-64 overflow-y-auto border-t border-border/60 px-2.5 py-2 text-xs">
          <MessageResponse className="text-foreground/80">{report.body}</MessageResponse>
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function PromptSectionCard({ section }: { readonly section: PromptSection }) {
  return (
    <Collapsible className="rounded-md border border-border/60 bg-muted/30">
      <CollapsibleTrigger className="group flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-accent/40">
        <span
          className="min-w-0 flex-1 truncate font-medium text-foreground/80"
          title={section.title}
        >
          {section.title}
        </span>
        <Chevron />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <pre className="max-h-64 overflow-y-auto border-t border-border/60 px-2.5 py-2 text-xs whitespace-pre-wrap text-muted-foreground">
          {section.body}
        </pre>
      </CollapsibleContent>
    </Collapsible>
  );
}

type TabId = "reports" | "prompt" | "rules" | "skills";

/**
 * Rich-content tabs fed by the session's stored system nodes — sub-agent
 * reports, the agent's own system prompt, rules and skills. The drawer is
 * the only consumer; paging the full history here is what surfaces reports
 * recorded mid-conversation (system nodes sit at the oldest page).
 */
export function SessionContextTabs({ session }: { readonly session: SessionSummary }) {
  const historyQuery = useHistory(session.id, session.agent, session.node);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = historyQuery;

  // Page to the oldest message so every system node is parsed — the shared
  // history cache absorbs the cost for sessions the chat already loaded.
  useEffect(() => {
    if (hasNextPage === true && !isFetchingNextPage) void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const history = useMemo(() => flattenHistory(historyQuery.data), [historyQuery.data]);
  const context = useMemo(
    () => parseSystemContext(history.filter((m) => m.role === "system")),
    [history],
  );

  // Optional fields the parser may not provide yet — read defensively.
  const { reports, rules, skills, sections } = useMemo(() => {
    const extras = context as unknown as Record<string, unknown>;
    return {
      reports: toList<Report>(context.reports as ReadonlyArray<unknown>, toReport),
      rules: toList<ContextEntry>(context.rules as ReadonlyArray<unknown>, toEntry),
      skills: toList<ContextEntry>(extras["skills"], toEntry),
      sections: toList<PromptSection>(extras["promptSections"], toSection),
    };
  }, [context]);

  const hasPrompt = context.promptText !== "" || sections.length > 0;
  const tabs: ReadonlyArray<{ id: TabId; label: string; count?: number }> = [
    { id: "reports", label: "Reports", count: reports.length },
    { id: "prompt", label: "Prompt" },
    { id: "rules", label: "Rules", count: rules.length },
    { id: "skills", label: "Skills", count: skills.length },
  ];
  const visible = tabs.filter((tab) => (tab.id === "prompt" ? hasPrompt : (tab.count ?? 0) > 0));

  if (historyQuery.isPending || (visible.length === 0 && hasNextPage === true)) {
    return (
      <div className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
        <Spinner className="size-3.5" />
        Loading session context…
      </div>
    );
  }
  if (historyQuery.isError) {
    return (
      <p className="mt-3 text-xs text-muted-foreground">
        Couldn&apos;t load the session&apos;s stored context.
      </p>
    );
  }
  if (visible.length === 0) return null;

  return (
    <Tabs defaultValue={visible[0]?.id} className="mt-3">
      <TabsList className="w-full justify-start gap-0.5 overflow-x-auto">
        {visible.map((tab) => (
          <TabsTrigger key={tab.id} value={tab.id} className="shrink-0 px-2.5 py-1 text-xs">
            {tab.label}
            {tab.count !== undefined && tab.count > 0 && (
              <span className="text-muted-foreground/70">{tab.count}</span>
            )}
          </TabsTrigger>
        ))}
      </TabsList>

      {visible.some((t) => t.id === "reports") && (
        <TabsContent value="reports" className="mt-2">
          <div className="flex max-h-80 flex-col gap-1.5 overflow-y-auto">
            {reports.map((report, i) => (
              <ReportCard key={`${report.title}-${i}`} report={report} />
            ))}
          </div>
        </TabsContent>
      )}

      {visible.some((t) => t.id === "prompt") && (
        <TabsContent value="prompt" className="mt-2">
          <div className="flex max-h-80 flex-col gap-1.5 overflow-y-auto">
            {sections.map((section, i) => (
              <PromptSectionCard key={`${section.title}-${i}`} section={section} />
            ))}
            {context.promptText !== "" &&
              (sections.length > 0 ? (
                <PromptSectionCard section={{ title: "Full prompt", body: context.promptText }} />
              ) : (
                <pre className="rounded-md bg-muted/40 p-2.5 text-xs whitespace-pre-wrap text-muted-foreground">
                  {context.promptText}
                </pre>
              ))}
          </div>
        </TabsContent>
      )}

      {visible.some((t) => t.id === "rules") && (
        <TabsContent value="rules" className="mt-2">
          <div className="flex max-h-80 flex-col gap-1.5 overflow-y-auto">
            {rules.map((rule, i) => (
              <EntryCard key={`${rule.title}-${i}`} entry={rule} />
            ))}
          </div>
        </TabsContent>
      )}

      {visible.some((t) => t.id === "skills") && (
        <TabsContent value="skills" className="mt-2">
          <div className="flex max-h-80 flex-col gap-1.5 overflow-y-auto">
            {skills.map((skill, i) => (
              <EntryCard key={`${skill.title}-${i}`} entry={skill} />
            ))}
          </div>
        </TabsContent>
      )}
    </Tabs>
  );
}
