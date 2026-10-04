import { useStore } from "@tanstack/react-store";
import { useHotkey, type UseHotkeyOptions } from "@tanstack/react-hotkeys";
import { settingsStore, type SepiaSettings } from "./settings";

export interface KeybindDef {
  readonly id: string;
  readonly group: string;
  readonly label: string;
  /** TanStack hotkey string, e.g. "Mod+K" / "Shift+Slash". */
  readonly def: string;
}

/** Every bindable action — settings overrides/disables live in settingsStore. */
export const KEYBINDS: ReadonlyArray<KeybindDef> = [
  { id: "session.new", group: "Sessions", label: "New session", def: "N" },
  { id: "nav.down", group: "Sessions", label: "Next session", def: "ArrowDown" },
  { id: "nav.up", group: "Sessions", label: "Previous session", def: "ArrowUp" },
  { id: "nav.collapse", group: "Sessions", label: "Collapse group", def: "ArrowLeft" },
  { id: "nav.expand", group: "Sessions", label: "Expand group", def: "ArrowRight" },
  { id: "filter.focus", group: "App", label: "Focus filter", def: "Mod+K" },
  { id: "filter.clear", group: "App", label: "Clear filter", def: "Escape" },
  { id: "app.sidebar", group: "App", label: "Toggle sidebar", def: "Mod+B" },
  { id: "app.settings", group: "App", label: "Settings", def: "Mod+Comma" },
  { id: "app.keybinds", group: "App", label: "Keyboard shortcuts", def: "Shift+Slash" },
];

export const keybindDef = (id: string): KeybindDef | undefined =>
  KEYBINDS.find((keybind) => keybind.id === id);

/** Effective hotkey — settings override wins; null means disabled. */
export const resolveKey = (settings: SepiaSettings, id: string): string | null => {
  const override = settings.keybinds[id];
  if (override === null) return null;
  if (typeof override === "string") return override;
  return keybindDef(id)?.def ?? null;
};

/** Display tokens for a hotkey string: "Mod+K" → ["Ctrl", "K"] / ["⌘", "K"]. */
export const formatKey = (key: string, modKey: string): string[] =>
  key.split("+").map((part) => (part === "Mod" ? modKey : part === "Slash" ? "/" : part));

/** useHotkey driven by the keybind registry — honors overrides + disables. */
export const useAppHotkey = (
  id: string,
  callback: Parameters<typeof useHotkey>[1],
  options?: UseHotkeyOptions,
): void => {
  const keybinds = useStore(settingsStore, (state) => state.keybinds);
  const override = keybinds[id];
  const key = override === null ? null : (override ?? keybindDef(id)?.def ?? null);
  // A disabled binding keeps a dead registration — `enabled:false` suppresses firing.
  useHotkey((key ?? "Mod+Shift+F24") as Parameters<typeof useHotkey>[0], callback, {
    ...options,
    enabled: key !== null,
  });
};
