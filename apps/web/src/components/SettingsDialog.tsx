import { useEffect, useRef, useState } from "react";
import { useHotkeyRecorder } from "@tanstack/react-hotkeys";
import { useHasKeyboard } from "../lib/keyboard";
import { useStore } from "@tanstack/react-store";
import { settingsStore, setSettings, type SepiaSettings } from "../lib/settings";
import { sepiaStore } from "../lib/store";
import { KEYBINDS, formatKey, keybindConflicts, resolveKey } from "../lib/keybinds";
import { Kbd } from "./ui/kbd";
import { Button } from "./ui/button";
import { Switch } from "./ui/switch";
import {
  isPushSupported,
  pushState,
  subscribePush,
  unsubscribePush,
  updatePushPrefs,
} from "../lib/push";
import { NodesSection } from "./NodesSection";
import { AgentsSection } from "./settings/AgentsSection";
import { ClientSection } from "./settings/ClientSection";
import { CredentialsSection } from "./settings/CredentialsSection";
import { DesktopSection } from "./settings/DesktopSection";
import { ModelsSection } from "./settings/ModelsSection";
import { SidebarSection } from "./settings/SidebarSection";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";
import { ScrollArea } from "./ui/scroll-area";
import { Select, SelectContent, SelectItem, SelectTrigger } from "./ui/select";

interface SettingsDialogProps {
  readonly open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Keyboard shortcuts — record a new sequence, disable, or restore defaults. */
function KeyboardSection() {
  const settings = useStore(settingsStore);
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const recorder = useHotkeyRecorder({
    onRecord: (hotkey) => {
      if (recordingId !== null) {
        setSettings({ keybinds: { ...settings.keybinds, [recordingId]: String(hotkey) } });
      }
      setRecordingId(null);
    },
    // Backspace/Delete while recording disables the binding entirely.
    onClear: () => {
      if (recordingId !== null) {
        setSettings({ keybinds: { ...settings.keybinds, [recordingId]: null } });
      }
      setRecordingId(null);
    },
    onCancel: () => setRecordingId(null),
  });

  const groups = [...new Set(KEYBINDS.map((keybind) => keybind.group))];

  return (
    <section data-spy="keyboard" className="flex scroll-mt-2 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">Keyboard</h3>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => setSettings({ keybinds: {} })}
          disabled={Object.keys(settings.keybinds).length === 0}
        >
          Restore defaults
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        Click a shortcut to rebind it — Escape cancels, Backspace disables.
      </p>
      {groups.map((group) => (
        <div key={group} className="flex flex-col">
          <span className="py-1 text-xs font-medium text-muted-foreground">{group}</span>
          <div className="divide-y divide-border/50 rounded-lg border border-border">
            {KEYBINDS.filter((keybind) => keybind.group === group).map((keybind) => {
              const key = resolveKey(settings, keybind.id);
              const conflicts = keybindConflicts(settings, keybind.id);
              const recording = recordingId === keybind.id && recorder.isRecording;
              return (
                <div key={keybind.id} className="flex items-center gap-3 px-3 py-2">
                  <div className="flex-1">
                    <span className="text-sm">{keybind.label}</span>
                    {conflicts.length > 0 && (
                      <span className="block text-xs text-amber-600 dark:text-amber-400">
                        Conflicts with {conflicts.map((other) => other.label).join(", ")}
                      </span>
                    )}
                  </div>
                  {key === null ? (
                    <span className="text-xs text-muted-foreground">Disabled</span>
                  ) : (
                    <span className="flex gap-1">
                      {formatKey(recording ? (recorder.recordedHotkey ?? key) : key, MOD_KEY).map(
                        (part) => (
                          <Kbd key={part}>{part}</Kbd>
                        ),
                      )}
                    </span>
                  )}
                  <Button
                    variant="ghost"
                    size="xs"
                    onClick={() => {
                      setRecordingId(keybind.id);
                      recorder.startRecording();
                    }}
                  >
                    {recording ? (recorder.recordedHotkey ?? "Press keys…") : "Change"}
                  </Button>
                  <Switch
                    checked={key !== null}
                    onCheckedChange={(value) =>
                      setSettings({
                        keybinds: {
                          ...settings.keybinds,
                          [keybind.id]: value ? keybind.def : null,
                        },
                      })
                    }
                    aria-label={`${key === null ? "Enable" : "Disable"} ${keybind.label}`}
                    title={key === null ? "Enable" : "Disable"}
                  />
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </section>
  );
}

/** Push notifications — master subscribe toggle + per-event prefs. */
function NotificationsSection() {
  const settings = useStore(settingsStore);
  const n = settings.notifications;
  const [busy, setBusy] = useState<string | null>(null);
  const supported = isPushSupported();
  const denied = supported && Notification.permission === "denied";

  // Reconcile the stored toggle with the real subscription — subs can be
  // pruned server-side (404/410) or cleared in browser settings.
  useEffect(() => {
    let cancelled = false;
    void pushState().then((state) => {
      if (cancelled) return;
      const current = settingsStore.state.notifications;
      if (state === "subscribed" && !current.enabled) {
        setSettings({ notifications: { ...current, enabled: true } });
      } else if (current.enabled && state === "granted") {
        // Browser dropped the subscription — re-register to repair.
        void updatePushPrefs({ done: current.done, permission: current.permission });
      } else if (current.enabled && state !== "subscribed") {
        setSettings({ notifications: { ...current, enabled: false } });
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const setNotif = (patch: Partial<typeof n>): void => {
    const next = { ...n, ...patch };
    setSettings({ notifications: next });
    if (next.enabled) void updatePushPrefs({ done: next.done, permission: next.permission });
  };

  const toggleMaster = (enabled: boolean): void => {
    if (busy !== null) return;
    setBusy("master");
    void (async () => {
      if (enabled) {
        const permission =
          Notification.permission === "granted"
            ? "granted"
            : await Notification.requestPermission();
        if (permission === "granted") {
          const ok = await subscribePush({ done: n.done, permission: n.permission });
          if (ok) setSettings({ notifications: { ...n, enabled: true } });
        }
      } else {
        await unsubscribePush();
        setSettings({ notifications: { ...n, enabled: false } });
      }
      setBusy(null);
    })();
  };

  const rows = [
    { key: "done" as const, label: "Session finished", hint: "When an agent run ends or errors." },
    {
      key: "permission" as const,
      label: "Approval needed",
      hint: "When an agent waits on a permission decision.",
    },
  ];

  return (
    <section data-spy="notifications" className="flex scroll-mt-2 flex-col gap-2">
      <h3 className="text-sm font-medium">Notifications</h3>
      {!supported ? (
        <p className="text-xs text-muted-foreground">Push isn&apos;t supported in this browser.</p>
      ) : denied ? (
        <p className="text-xs text-muted-foreground">
          Notifications are blocked — allow them in your browser&apos;s site settings.
        </p>
      ) : (
        <div className="divide-y divide-border/50 rounded-lg border border-border">
          <div className="flex items-center gap-3 px-3 py-2.5">
            <div className="flex-1">
              <span className="block text-sm font-medium">Push notifications</span>
              <span className="block text-xs text-muted-foreground">
                Notify this browser even when the tab isn&apos;t focused.
              </span>
            </div>
            <Switch
              checked={n.enabled}
              onCheckedChange={toggleMaster}
              disabled={busy === "master"}
            />
          </div>
          {n.enabled &&
            rows.map((row) => (
              <div key={row.key} className="flex items-center gap-3 px-3 py-2.5">
                <div className="flex-1">
                  <span className="block text-sm">{row.label}</span>
                  <span className="block text-xs text-muted-foreground">{row.hint}</span>
                </div>
                <Switch
                  checked={n[row.key]}
                  onCheckedChange={(value) => setNotif({ [row.key]: value })}
                />
              </div>
            ))}
        </div>
      )}
    </section>
  );
}

const SECTIONS = [
  { id: "general", label: "General" },
  { id: "client", label: "Client" },
  { id: "credentials", label: "Credentials" },
  { id: "nodes", label: "Nodes" },
  { id: "agents", label: "Agents" },
  { id: "models", label: "Models" },
  { id: "desktop", label: "Desktop" },
  { id: "sidebar", label: "Sidebar" },
  { id: "keyboard", label: "Keyboard" },
  { id: "notifications", label: "Notifications" },
] as const;

const MOD_KEY = navigator.platform.toUpperCase().includes("MAC") ? "⌘" : "Ctrl";

export function SettingsDialog({ open, onOpenChange }: SettingsDialogProps) {
  const hasKeyboard = useHasKeyboard();
  const settings = useStore(settingsStore);
  const settingsSection = useStore(sepiaStore, (state) => state.settingsSection);
  useEffect(() => {
    if (!open || settingsSection === null) return;
    // Two frames — the dialog mounts + lays out its scroll area before
    // scrollIntoView, or it clamps to the top.
    const raf = requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        contentRef.current
          ?.querySelector(`[data-spy="${settingsSection}"]`)
          ?.scrollIntoView({ behavior: "smooth", block: "start" });
        sepiaStore.setState((prev) => ({ ...prev, settingsSection: null }));
      });
    });
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, settingsSection]);
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
          <DialogDescription>Client-local preferences for this Sepia instance.</DialogDescription>
        </DialogHeader>
        <div className="flex min-h-0 flex-1 gap-4 max-md:flex-col">
          <ScrollArea className="w-36 shrink-0 max-md:w-auto">
            <nav
              className="flex flex-col gap-0.5 max-md:w-max max-md:min-w-full max-md:flex-row max-md:gap-1 max-md:pb-1 p-1"
              aria-label="Settings sections"
            >
              {SECTIONS.filter((s) => s.id !== "keyboard" || hasKeyboard).map((section) => (
                <button
                  key={section.id}
                  type="button"
                  onClick={() => scrollTo(section.id)}
                  aria-current={active === section.id ? "true" : undefined}
                  className={`rounded-md px-3 py-1.5 text-left text-sm font-medium whitespace-nowrap transition-colors ${
                    active === section.id
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:bg-accent/60 hover:text-foreground"
                  }`}
                >
                  {section.label}
                </button>
              ))}
            </nav>
          </ScrollArea>
          <ScrollArea className="min-h-0 flex-1">
            {/* p-2 keeps outward focus rings inside the scroll bounds. */}
            <div ref={contentRef} className="flex flex-col gap-8 p-2 pr-3">
              <section data-spy="general" className="flex scroll-mt-2 flex-col gap-4">
                <div className="flex flex-col gap-1.5">
                  <label className="text-sm font-medium" htmlFor="settings-theme">
                    Theme
                  </label>
                  <Select
                    value={settings.theme}
                    onValueChange={(value) =>
                      setSettings({ theme: value as SepiaSettings["theme"] })
                    }
                  >
                    <SelectTrigger id="settings-theme" aria-label="Theme" className="w-full">
                      {settings.theme.charAt(0).toUpperCase() + settings.theme.slice(1)}
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="dark">Dark</SelectItem>
                      <SelectItem value="light">Light</SelectItem>
                      <SelectItem value="system">System</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </section>
              <ClientSection />
              <CredentialsSection />
              <NodesSection />
              <AgentsSection scrollTo={scrollTo} />
              <ModelsSection scrollTo={scrollTo} />
              <DesktopSection scrollTo={scrollTo} />
              <SidebarSection />
              {hasKeyboard && <KeyboardSection />}
              <NotificationsSection />
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  );
}
