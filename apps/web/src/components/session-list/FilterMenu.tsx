import { FilterHorizontalIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { AgentInfo } from "../../lib/types";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import type { DateFilter, SortKey, StatusFilter } from "./FilterBar";

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
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Agents</DropdownMenuLabel>
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
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup
          value={dateFilter}
          onValueChange={(v) => onDateFilterChange(v as DateFilter)}
        >
          <DropdownMenuLabel>Recency</DropdownMenuLabel>
          <DropdownMenuRadioItem value="all" closeOnClick={false}>
            Any time
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="day" closeOnClick={false}>
            Today
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="week" closeOnClick={false}>
            Last 7 days
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="month" closeOnClick={false}>
            Last 30 days
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup
          value={statusFilter}
          onValueChange={(v) => onStatusFilterChange(v as StatusFilter)}
        >
          <DropdownMenuLabel>Status</DropdownMenuLabel>
          <DropdownMenuRadioItem value="all" closeOnClick={false}>
            Any status
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="free" closeOnClick={false}>
            Free
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="locked" closeOnClick={false}>
            Locked
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup value={sort} onValueChange={(v) => onSortChange(v as SortKey)}>
          <DropdownMenuLabel>Sort</DropdownMenuLabel>
          <DropdownMenuRadioItem value="newest" closeOnClick={false}>
            Newest
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="oldest" closeOnClick={false}>
            Oldest
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="title" closeOnClick={false}>
            Title
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
