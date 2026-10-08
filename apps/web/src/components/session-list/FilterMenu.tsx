import {
  Add01Icon,
  BotIcon,
  Clock01Icon,
  FilterHorizontalIcon,
  StatusIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { setSettingsOpen } from "../../lib/store";
import type { AgentInfo } from "../../lib/types";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import type { DateFilter, StatusFilter } from "./FilterBar";

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

/** Muted right-hand hint showing the active choice on a submenu trigger. */
function Hint({ children }: { readonly children: React.ReactNode }) {
  return <span className="ml-auto text-xs text-muted-foreground">{children}</span>;
}

interface FilterMenuProps {
  readonly agents: ReadonlyArray<AgentInfo>;
  /** false while the roster query is pending — the empty state waits for it. */
  readonly agentsLoaded: boolean;
  readonly agentFilter: ReadonlyArray<string>;
  readonly dateFilter: DateFilter;
  readonly statusFilter: StatusFilter;
  onToggleAgent: (id: string, checked: boolean) => void;
  onDateFilterChange: (value: DateFilter) => void;
  onStatusFilterChange: (value: StatusFilter) => void;
}

export function FilterMenu({
  agents,
  agentsLoaded,
  agentFilter,
  dateFilter,
  statusFilter,
  onToggleAgent,
  onDateFilterChange,
  onStatusFilterChange,
}: FilterMenuProps) {
  const active =
    agentFilter.length + (dateFilter === "all" ? 0 : 1) + (statusFilter === "all" ? 0 : 1);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className="relative shrink-0"
            aria-label="Filters"
            title="Filters"
          />
        }
      >
        <HugeiconsIcon icon={FilterHorizontalIcon} strokeWidth={2} />
        {active > 0 && (
          <span className="absolute top-0.5 right-0.5 size-1.5 rounded-full bg-primary" />
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
            {/* Empty roster = no node reachable — point at the fix instead
                of opening a blank submenu. Gated on a settled query so it
                can't flash while the roster is still loading. */}
            {agentsLoaded && agents.length === 0 && (
              <>
                <DropdownMenuItem disabled>
                  <span className="text-muted-foreground">No agents — connect a node</span>
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setSettingsOpen(true, "nodes")}>
                  <HugeiconsIcon icon={Add01Icon} strokeWidth={2} />
                  Add a node
                </DropdownMenuItem>
              </>
            )}
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
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
