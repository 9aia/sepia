import type { RefObject } from "react";
import type { AgentInfo } from "../../lib/types";
import { Input } from "../ui/input";
import { Kbd, KbdGroup } from "../ui/kbd";
import { FilterMenu } from "./FilterMenu";

export type DateFilter = "all" | "day" | "week" | "month";
export type StatusFilter = "all" | "free" | "locked";
export type SortKey = "newest" | "oldest" | "title";

interface FilterBarProps {
  readonly agents: ReadonlyArray<AgentInfo>;
  readonly filter: string;
  readonly filterRef: RefObject<HTMLInputElement | null>;
  readonly modKey: string;
  readonly agentFilter: ReadonlyArray<string>;
  readonly dateFilter: DateFilter;
  readonly statusFilter: StatusFilter;
  readonly sort: SortKey;
  onFilterChange: (filter: string) => void;
  onToggleAgent: (id: string, checked: boolean) => void;
  onDateFilterChange: (value: DateFilter) => void;
  onStatusFilterChange: (value: StatusFilter) => void;
  onSortChange: (value: SortKey) => void;
}

export function FilterBar({
  agents,
  filter,
  filterRef,
  modKey,
  agentFilter,
  dateFilter,
  statusFilter,
  sort,
  onFilterChange,
  onToggleAgent,
  onDateFilterChange,
  onStatusFilterChange,
  onSortChange,
}: FilterBarProps) {
  return (
    <div className="flex items-center gap-1.5 px-2 pb-2">
      <div className="relative min-w-0 flex-1">
        <Input
          type="search"
          className="pr-16"
          placeholder="Filter sessions…"
          aria-label="Filter sessions"
          ref={filterRef}
          value={filter}
          onChange={(event) => onFilterChange(event.target.value)}
        />
        <KbdGroup
          className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2"
          aria-hidden="true"
        >
          <Kbd>{modKey}</Kbd>
          <Kbd>K</Kbd>
        </KbdGroup>
      </div>
      <FilterMenu
        agents={agents}
        agentFilter={agentFilter}
        dateFilter={dateFilter}
        statusFilter={statusFilter}
        sort={sort}
        onToggleAgent={onToggleAgent}
        onDateFilterChange={onDateFilterChange}
        onStatusFilterChange={onStatusFilterChange}
        onSortChange={onSortChange}
      />
    </div>
  );
}
