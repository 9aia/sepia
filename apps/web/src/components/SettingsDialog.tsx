import { useStore } from "@tanstack/react-store";
import { settingsStore, setSettings, type AgentModelPref } from "../lib/settings";
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
          <div className="flex flex-col gap-2 border-t border-border pt-4">
            <label className="text-sm font-medium">Models</label>
            <p className="text-xs text-muted-foreground">
              Spawn-time model per agent — applied when the session&apos;s agent starts. Auto mode
              also sends the fallback list (devin&apos;s refusal-fallback).
            </p>
            {agents.map((agent) => {
              const pref: AgentModelPref = settings.models[agent.id] ?? {
                model: "",
                fallbacks: "",
                mode: "auto",
              };
              const update = (patch: Partial<AgentModelPref>): void =>
                setSettings({
                  models: { ...settings.models, [agent.id]: { ...pref, ...patch } },
                });
              return (
                <div
                  key={agent.id}
                  className="flex flex-col gap-2 rounded-lg border border-border p-3"
                >
                  <span className="text-sm font-medium">{agent.label}</span>
                  <div className="grid gap-2 sm:grid-cols-2">
                    <Input
                      placeholder="Model (empty = agent default)"
                      aria-label={`${agent.label} model`}
                      value={pref.model}
                      onChange={(event) => update({ model: event.target.value })}
                    />
                    <Select
                      value={pref.mode}
                      onValueChange={(v) => update({ mode: v as AgentModelPref["mode"] })}
                    >
                      <SelectTrigger aria-label={`${agent.label} fallback mode`}>
                        {pref.mode === "auto" ? "Auto fallback" : "Manual"}
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="auto">Auto fallback</SelectItem>
                        <SelectItem value="manual">Manual</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  {pref.mode === "auto" && (
                    <Input
                      placeholder="Fallback models, comma-separated"
                      aria-label={`${agent.label} fallback models`}
                      value={pref.fallbacks}
                      onChange={(event) => update({ fallbacks: event.target.value })}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
