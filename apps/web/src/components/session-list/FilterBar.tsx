import type { RefObject } from "react";
import type { AgentInfo } from "../../lib/types";
import { Input } from "../ui/input";
import { Kbd, KbdGroup } from "../ui/kbd";
import { FilterSelect } from "./FilterSelect";

export type DateFilter = "all" | "day" | "week" | "month";
export type StatusFilter = "all" | "free" | "locked";
export type SortKey = "newest" | "oldest" | "title";

interface FilterBarProps {
  readonly agents: ReadonlyArray<AgentInfo>;
  readonly filter: string;
  readonly filterRef: RefObject<HTMLInputElement | null>;
  readonly modKey: string;
  readonly agentFilter: string;
  readonly dateFilter: DateFilter;
  readonly statusFilter: StatusFilter;
  readonly sort: SortKey;
  onFilterChange: (filter: string) => void;
  onAgentFilterChange: (value: string) => void;
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
  onAgentFilterChange,
  onDateFilterChange,
  onStatusFilterChange,
  onSortChange,
}: FilterBarProps) {
  return (
    <>
      <div className="relative mx-2 mb-2">
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

      <div className="flex gap-1.5 px-2 pb-2">
        <FilterSelect
          label="Filter by agent"
          value={agentFilter}
          onChange={onAgentFilterChange}
          options={[
            { value: "all", label: "All agents" },
            ...agents.map((a) => ({ value: a.id, label: a.label })),
          ]}
        />
        <FilterSelect
          label="Filter by recency"
          value={dateFilter}
          onChange={(v) => onDateFilterChange(v as DateFilter)}
          options={[
            { value: "all", label: "Any time" },
            { value: "day", label: "Today" },
            { value: "week", label: "Last 7 days" },
            { value: "month", label: "Last 30 days" },
          ]}
        />
        <FilterSelect
          label="Filter by lock status"
          value={statusFilter}
          onChange={(v) => onStatusFilterChange(v as StatusFilter)}
          options={[
            { value: "all", label: "Any status" },
            { value: "free", label: "Free" },
            { value: "locked", label: "Locked" },
          ]}
        />
        <FilterSelect
          label="Sort sessions"
          value={sort}
          onChange={(v) => onSortChange(v as SortKey)}
          options={[
            { value: "newest", label: "Newest" },
            { value: "oldest", label: "Oldest" },
            { value: "title", label: "Title" },
          ]}
        />
      </div>
    </>
  );
}
