import { useState } from "react";
import { useStore } from "@tanstack/react-store";
import { cn } from "cn";
import { settingsStore, setSettings } from "../lib/settings";
import { useAgents } from "../hooks/query/useAgents";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Input } from "./ui/input";
import { Kbd } from "./ui/kbd";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

const SECTIONS = ["General", "Shortcuts"] as const;
type Section = (typeof SECTIONS)[number];

function ShortcutRow({ keys, action }: { readonly keys: string[]; readonly action: string }) {
  return (
    <div className="flex items-center justify-between py-1.5 text-sm">
      <span>{action}</span>
      <span className="flex gap-1">
        {keys.map((key) => (
          <Kbd key={key}>{key}</Kbd>
        ))}
      </span>
    </div>
  );
}

interface SettingsDialogProps {
  readonly open: boolean;
  readonly modKey: string;
  onOpenChange: (open: boolean) => void;
}

export function SettingsDialog({ open, modKey, onOpenChange }: SettingsDialogProps) {
  const { data: agents = [] } = useAgents();
  const settings = useStore(settingsStore);
  const [section, setSection] = useState<Section>("General");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <DialogHeader className="border-b border-border p-4 pb-3">
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Browser-local preferences for this Sepia instance.</DialogDescription>
        </DialogHeader>
        <div className="flex min-h-[280px]">
          <nav className="w-36 shrink-0 border-r border-border p-2">
            {SECTIONS.map((item) => (
              <button
                key={item}
                type="button"
                onClick={() => setSection(item)}
                className={cn(
                  "w-full rounded-md px-2.5 py-1.5 text-left text-sm text-muted-foreground hover:bg-accent",
                  item === section && "bg-accent font-medium text-foreground",
                )}
              >
                {item}
              </button>
            ))}
          </nav>
          <div className="flex-1 p-4">
            {section === "General" && (
              <div className="flex flex-col gap-4">
                <div className="flex flex-col gap-1.5">
                  <label className="text-sm font-medium" htmlFor="settings-agent">
                    Default agent
                  </label>
                  <Select
                    value={settings.defaultAgent ?? "__server__"}
                    onValueChange={(value) =>
                      setSettings({ defaultAgent: value === "__server__" ? null : value })
                    }
                  >
                    <SelectTrigger id="settings-agent" aria-label="Default agent">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__server__">Server default</SelectItem>
                      {agents.map((agent) => (
                        <SelectItem key={agent.id} value={agent.id}>
                          {agent.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">
                    Preselected when creating a session.
                  </p>
                </div>
                <div className="flex flex-col gap-1.5">
                  <label className="text-sm font-medium" htmlFor="settings-cwd">
                    Default directory
                  </label>
                  <Input
                    id="settings-cwd"
                    placeholder="Most recent session's directory"
                    value={settings.defaultCwd ?? ""}
                    onChange={(event) =>
                      setSettings({
                        defaultCwd: event.target.value.trim() === "" ? null : event.target.value,
                      })
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    Prefilled working directory; empty uses your most recent session's.
                  </p>
                </div>
              </div>
            )}
            {section === "Shortcuts" && (
              <div className="divide-y divide-border/50">
                <ShortcutRow keys={["↑", "↓"]} action="Navigate sessions" />
                <ShortcutRow keys={["←", "→"]} action="Collapse / expand group" />
                <ShortcutRow keys={["N"]} action="New session" />
                <ShortcutRow keys={[modKey, "K"]} action="Focus filter" />
                <ShortcutRow keys={[modKey, "B"]} action="Toggle sidebar" />
                <ShortcutRow keys={["Esc"]} action="Clear filter" />
              </div>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
