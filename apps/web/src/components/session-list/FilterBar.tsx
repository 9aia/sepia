import { useState, type RefObject } from "react";
import { useAppHotkey } from "../../lib/keybinds";
import { PanelLeftCloseIcon, Search01Icon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import type { AgentInfo } from "../../lib/types";
import { Button } from "../ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "../ui/popover";
import { Input } from "../ui/input";
import { Kbd, KbdGroup } from "../ui/kbd";
import { useSidebar } from "../ui/sidebar";
import { FilterMenu } from "./FilterMenu";
import { SortMenu } from "./SortMenu";

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
  const { toggleSidebar } = useSidebar();
  const [searchOpen, setSearchOpen] = useState(false);
  useAppHotkey("filter.focus", () => setSearchOpen(true), { preventDefault: true });
  return (
    <div className="flex items-center gap-1">
      <Popover
        open={searchOpen}
        onOpenChange={(open) => {
          setSearchOpen(open);
          if (open) requestAnimationFrame(() => filterRef.current?.focus());
        }}
      >
        <PopoverTrigger
          render={
            <Button
              variant="ghost"
              size="icon-xs"
              className="shrink-0"
              aria-label="Search sessions"
              title={`Search (${modKey}K)`}
            />
          }
        >
          <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 p-1.5">
          <div className="relative">
            <Input
              type="search"
              autoFocus
              className="pr-16"
              placeholder="Filter sessions…"
              aria-label="Filter sessions"
              ref={filterRef}
              value={filter}
              onChange={(event) => onFilterChange(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") setSearchOpen(false);
              }}
            />
            <KbdGroup
              className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2"
              aria-hidden="true"
            >
              <Kbd>{modKey}</Kbd>
              <Kbd>K</Kbd>
            </KbdGroup>
          </div>
        </PopoverContent>
      </Popover>
      <FilterMenu
        agents={agents}
        agentFilter={agentFilter}
        dateFilter={dateFilter}
        statusFilter={statusFilter}
        onToggleAgent={onToggleAgent}
        onDateFilterChange={onDateFilterChange}
        onStatusFilterChange={onStatusFilterChange}
      />
      <SortMenu sort={sort} onSortChange={onSortChange} />
      <Button
        variant="ghost"
        size="icon-xs"
        className="shrink-0"
        aria-label="Close sidebar"
        title="Close sidebar"
        onClick={toggleSidebar}
      >
        <HugeiconsIcon icon={PanelLeftCloseIcon} strokeWidth={2} />
      </Button>
    </div>
  );
}
