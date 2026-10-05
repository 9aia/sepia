import { Fragment, type ReactNode } from "react";
import { ChevronDownIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { CatalogModel } from "../../lib/catalog";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";

/** Small muted count badge — the `h-4 text-[10px]` settings convention. */
export function CountBadge({ count, title }: { readonly count: number; readonly title: string }) {
  return (
    <Badge variant="secondary" className="h-4 shrink-0 px-1.5 text-[10px]" title={title}>
      {count}
    </Badge>
  );
}

/** The outline "disabled" marker parked rows carry. */
export function DisabledBadge() {
  return (
    <Badge variant="outline" className="h-4 shrink-0 px-1.5 text-[10px]">
      disabled
    </Badge>
  );
}

/**
 * A model id list — "Agent default · opus · sonnet" — with parked entries
 * struck through so a disabled model still shows where it came from.
 */
export function ModelList({ models }: { readonly models: ReadonlyArray<CatalogModel> }) {
  return (
    <span className="block truncate text-xs text-muted-foreground">
      {models.map((model, index) => (
        <Fragment key={model.key}>
          {index > 0 && " · "}
          <span
            className={model.enabled ? "" : "opacity-60 line-through"}
            title={model.enabled ? undefined : "Parked on this node+agent"}
          >
            {model.label}
          </span>
        </Fragment>
      ))}
    </span>
  );
}

/**
 * A roster row with a collapsible detail — the accordion idiom (chevron
 * trigger, content below the row inside the same bordered list). `leading`
 * slots a status affordance before the title column (the nodes'
 * StatusDot); `trailing` the right-side controls (switches, menus). Rows
 * without `children` render no chevron.
 */
export function CatalogRow({
  label,
  title,
  badges,
  subline,
  leading,
  trailing,
  dimmed = false,
  children,
}: {
  /** Accessible name for the disclosure chevron. */
  readonly label: string;
  readonly title: ReactNode;
  readonly badges?: ReactNode;
  readonly subline?: ReactNode;
  readonly leading?: ReactNode;
  readonly trailing?: ReactNode;
  /** Parked rows dim the label column — the nodes' `opacity-60` convention. */
  readonly dimmed?: boolean;
  /** Detail content — omitted renders a bare row. */
  readonly children?: ReactNode;
}) {
  return (
    <Collapsible>
      <div className="flex items-center gap-3 px-3 py-2.5">
        {children !== undefined && (
          <CollapsibleTrigger
            className="group -ml-1.5 flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            aria-label={`Details for ${label}`}
            title="Details"
          >
            <HugeiconsIcon
              icon={ChevronDownIcon}
              strokeWidth={2}
              className="size-3.5 transition-transform group-data-[panel-open]:rotate-180"
            />
          </CollapsibleTrigger>
        )}
        {leading}
        <div className={`min-w-0 flex-1${dimmed ? " opacity-60" : ""}`}>
          <span className="flex items-center gap-1.5">
            <span className="truncate text-sm font-medium">{title}</span>
            {badges}
          </span>
          {subline !== undefined && (
            <span className="block truncate text-xs text-muted-foreground">{subline}</span>
          )}
        </div>
        {trailing}
      </div>
      {children !== undefined && (
        <CollapsibleContent>
          <div className="border-t border-border/50 py-2 pr-3 pl-12">{children}</div>
        </CollapsibleContent>
      )}
    </Collapsible>
  );
}

/** The roster's no-rows card — message plus the "connect a node" affordance. */
export function CatalogEmptyCard({
  title,
  body,
  onConnect,
}: {
  readonly title: string;
  readonly body: string;
  readonly onConnect: () => void;
}) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <span className="text-sm font-medium">{title}</span>
      <span className="text-xs text-muted-foreground">{body}</span>
      <div>
        <Button variant="secondary" size="xs" onClick={onConnect}>
          Add a node
        </Button>
      </div>
    </div>
  );
}
