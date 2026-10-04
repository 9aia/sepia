import {
  ArrowUpDownIcon,
  BotIcon,
  Clock01Icon,
  FilterHorizontalIcon,
  StatusIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { AgentInfo } from "../../lib/types";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import type { DateFilter, SortKey, StatusFilter } from "./FilterBar";

const DATE_LABELS: Record<DateFilter, string> = {
  all: "Any time",
  day: "Today",
  week: "Last 7 days",
  month: "Last 30 days",
};

const STATUS_LABELS: Record<StatusFilter, string> = {
  all: "Any status",
  free: "Free",
  locked: "Locked",
};

const SORT_LABELS: Record<SortKey, string> = {
  newest: "Newest",
  oldest: "Oldest",
  title: "Title",
};

/** Muted right-hand hint showing the active choice on a submenu trigger. */
function Hint({ children }: { readonly children: React.ReactNode }) {
  return <span className="ml-auto text-xs text-muted-foreground">{children}</span>;
}

interface FilterMenuProps {
  readonly agents: ReadonlyArray<AgentInfo>;
  readonly agentFilter: ReadonlyArray<string>;
  readonly dateFilter: DateFilter;
  readonly statusFilter: StatusFilter;
  readonly sort: SortKey;
  onToggleAgent: (id: string, checked: boolean) => void;
  onDateFilterChange: (value: DateFilter) => void;
  onStatusFilterChange: (value: StatusFilter) => void;
  onSortChange: (value: SortKey) => void;
}

export function FilterMenu({
  agents,
  agentFilter,
  dateFilter,
  statusFilter,
  sort,
  onToggleAgent,
  onDateFilterChange,
  onStatusFilterChange,
  onSortChange,
}: FilterMenuProps) {
  const active =
    agentFilter.length +
    (dateFilter === "all" ? 0 : 1) +
    (statusFilter === "all" ? 0 : 1) +
    (sort === "newest" ? 0 : 1);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="outline" className="h-9 shrink-0 gap-1.5 px-2" aria-label="Filters" />
        }
      >
        <HugeiconsIcon icon={FilterHorizontalIcon} strokeWidth={2} />
        {active > 0 && (
          <Badge variant="secondary" className="px-1">
            {active}
          </Badge>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <HugeiconsIcon icon={BotIcon} strokeWidth={2} />
            Agents
            {agentFilter.length > 0 && <Hint>{agentFilter.length}</Hint>}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            {agents.map((agent) => (
              <DropdownMenuCheckboxItem
                key={agent.id}
                checked={agentFilter.includes(agent.id)}
                onCheckedChange={(checked) => onToggleAgent(agent.id, checked)}
                closeOnClick={false}
              >
                {agent.label}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>

        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <HugeiconsIcon icon={Clock01Icon} strokeWidth={2} />
            Recency
            {dateFilter !== "all" && <Hint>{DATE_LABELS[dateFilter]}</Hint>}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            <DropdownMenuRadioGroup
              value={dateFilter}
              onValueChange={(v) => onDateFilterChange(v as DateFilter)}
            >
              {(["all", "day", "week", "month"] as const).map((value) => (
                <DropdownMenuRadioItem key={value} value={value} closeOnClick={false}>
                  {DATE_LABELS[value]}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>

        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <HugeiconsIcon icon={StatusIcon} strokeWidth={2} />
            Status
            {statusFilter !== "all" && <Hint>{STATUS_LABELS[statusFilter]}</Hint>}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            <DropdownMenuRadioGroup
              value={statusFilter}
              onValueChange={(v) => onStatusFilterChange(v as StatusFilter)}
            >
              {(["all", "free", "locked"] as const).map((value) => (
                <DropdownMenuRadioItem key={value} value={value} closeOnClick={false}>
                  {STATUS_LABELS[value]}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>

        <DropdownMenuSeparator />

        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <HugeiconsIcon icon={ArrowUpDownIcon} strokeWidth={2} />
            Sort
            {sort !== "newest" && <Hint>{SORT_LABELS[sort]}</Hint>}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            <DropdownMenuRadioGroup value={sort} onValueChange={(v) => onSortChange(v as SortKey)}>
              {(["newest", "oldest", "title"] as const).map((value) => (
                <DropdownMenuRadioItem key={value} value={value} closeOnClick={false}>
                  {SORT_LABELS[value]}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
