import { useEffect, useRef, useState } from "react";
import { useStore } from "@tanstack/react-store";
import { settingsStore, setSettings, type AgentModelPref } from "../lib/settings";
import { useAgents } from "../hooks/query/useAgents";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { Input } from "./ui/input";
import { ScrollArea } from "./ui/scroll-area";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";

interface SettingsDialogProps {
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}

const SECTIONS = [
  { id: "general", label: "General" },
  { id: "models", label: "Models" },
] as const;

export function SettingsDialog({ open, onOpenChange }: SettingsDialogProps) {
  const { data: agents = [] } = useAgents();
  const settings = useStore(settingsStore);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [active, setActive] = useState<string>("general");

  // Scrollspy — the nav tracks which section is in view inside the ScrollArea.
  useEffect(() => {
    if (!open) return;
    const viewport = contentRef.current?.closest('[data-slot="scroll-area-viewport"]');
    const sections = contentRef.current?.querySelectorAll("[data-spy]");
    if (viewport === null || viewport === undefined || sections === undefined) return;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            setActive(entry.target.getAttribute("data-spy") ?? active);
          }
        }
      },
      { root: viewport, rootMargin: "0px 0px -70% 0px" },
    );
    sections.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const scrollTo = (id: string): void => {
    contentRef.current
      ?.querySelector(`[data-spy="${id}"]`)
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
    setActive(id);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Browser-local preferences for this Sepia instance.</DialogDescription>
        </DialogHeader>
        <div className="flex min-h-0 flex-1 gap-4">
          <nav className="flex w-36 shrink-0 flex-col gap-0.5" aria-label="Settings sections">
            {SECTIONS.map((section) => (
              <button
                key={section.id}
                type="button"
                onClick={() => scrollTo(section.id)}
                aria-current={active === section.id ? "true" : undefined}
                className={`rounded-md px-3 py-1.5 text-left text-sm font-medium transition-colors ${
                  active === section.id
                    ? "bg-accent text-foreground"
                    : "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
                }`}
              >
                {section.label}
              </button>
            ))}
          </nav>
          <ScrollArea className="min-h-0 flex-1">
            <div ref={contentRef} className="flex flex-col gap-8 pr-3">
              <section data-spy="general" className="flex scroll-mt-2 flex-col gap-4">
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
              </section>
              <section data-spy="models" className="flex scroll-mt-2 flex-col gap-2">
                <h3 className="text-sm font-medium">Models</h3>
                <p className="text-xs text-muted-foreground">
                  Spawn-time model per agent — applied when the session&apos;s agent starts. Auto
                  mode also sends the fallback list (devin&apos;s refusal-fallback).
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
              </section>
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  );
}
