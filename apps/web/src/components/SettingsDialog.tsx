import { useStore } from "@tanstack/react-store";
import { settingsStore, setSettings } from "../lib/settings";
import { useAgents } from "../hooks/query/useAgents";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Input } from "./ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";

interface SettingsDialogProps {
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function SettingsDialog({ open, onOpenChange }: SettingsDialogProps) {
  const { data: agents = [] } = useAgents();
  const settings = useStore(settingsStore);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Browser-local preferences for this Sepia instance.</DialogDescription>
        </DialogHeader>
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
                {settings.defaultAgent === null
                  ? "Server default"
                  : (agents.find((a) => a.id === settings.defaultAgent)?.label ??
                    settings.defaultAgent)}
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
            <p className="text-xs text-muted-foreground">Preselected when creating a session.</p>
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
      </DialogContent>
    </Dialog>
  );
}
