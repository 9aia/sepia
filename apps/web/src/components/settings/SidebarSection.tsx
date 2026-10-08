import { GripVerticalIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { useStore } from "@tanstack/react-store";
import { setSettings, settingsStore } from "../../lib/settings";
import {
  defaultSidebarSections,
  SIDEBAR_SECTION_LABELS,
  SIDEBAR_SECTION_LIMITS,
  sidebarSectionLabel,
  type SidebarSectionConfig,
  type SidebarSectionId,
} from "../../lib/sidebar";
import { Sortable, SortableItem, SortableItemHandle } from "../reui/sortable";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Switch } from "../ui/switch";

/**
 * Sidebar section config — reorder via drag handle, rename, hide, and cap
 * the row count of flat sections. Writes straight to `settingsStore` (the
 * Sortable applies the reorder optimistically and commits on drop).
 */
export function SidebarSection() {
  const sections = useStore(settingsStore, (state) => state.sidebar.sections);

  const commit = (next: SidebarSectionConfig[]): void =>
    setSettings({ sidebar: { sections: next } });
  const update = (id: SidebarSectionId, patch: Partial<SidebarSectionConfig>): void =>
    commit(sections.map((s) => (s.id === id ? { ...s, ...patch } : s)));

  const isDefault = JSON.stringify(sections) === JSON.stringify(defaultSidebarSections());

  return (
    <section data-spy="sidebar" className="flex scroll-mt-2 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">Sidebar</h3>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => commit(defaultSidebarSections())}
          disabled={isDefault}
        >
          Restore defaults
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        Drag to reorder the sidebar sections; toggle a section off to hide it. The label field
        renames the section header, the number caps how many rows show before &quot;Show more&quot;.
      </p>
      <Sortable
        value={sections}
        onValueChange={commit}
        getItemValue={(section) => section.id}
        className="flex flex-col gap-1.5"
      >
        {sections.map((section) => {
          const label = sidebarSectionLabel(section);
          const defaultLimit = SIDEBAR_SECTION_LIMITS[section.id];
          return (
            <SortableItem key={section.id} value={section.id}>
              <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/40 px-2 py-1.5">
                <SortableItemHandle
                  tabIndex={0}
                  aria-label={`Reorder ${label}`}
                  title="Drag to reorder"
                  className="shrink-0 rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <HugeiconsIcon icon={GripVerticalIcon} strokeWidth={2} className="size-4" />
                </SortableItemHandle>
                <Input
                  aria-label={`${SIDEBAR_SECTION_LABELS[section.id]} label`}
                  placeholder={SIDEBAR_SECTION_LABELS[section.id]}
                  value={section.label ?? ""}
                  onChange={(event) =>
                    update(section.id, {
                      label: event.target.value.trim() === "" ? undefined : event.target.value,
                    })
                  }
                  className="h-7 min-w-0 flex-1 text-sm"
                />
                {defaultLimit !== undefined && (
                  <Input
                    type="number"
                    min={1}
                    aria-label={`${label} row limit`}
                    title="Rows before 'Show more'"
                    placeholder={String(defaultLimit)}
                    value={section.limit ?? ""}
                    onChange={(event) => {
                      const value = Number(event.target.value);
                      update(section.id, {
                        limit:
                          event.target.value !== "" && Number.isFinite(value) && value > 0
                            ? Math.floor(value)
                            : undefined,
                      });
                    }}
                    className="h-7 w-16 shrink-0 text-sm"
                  />
                )}
                <Switch
                  aria-label={`Show ${label}`}
                  checked={section.enabled}
                  onCheckedChange={(checked) => update(section.id, { enabled: checked })}
                />
              </div>
            </SortableItem>
          );
        })}
      </Sortable>
    </section>
  );
}
