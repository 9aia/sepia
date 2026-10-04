import { useStore } from "@tanstack/react-store";
import { useHotkey, type UseHotkeyOptions } from "@tanstack/react-hotkeys";
import { areHotkeysEqual, type RegisterableHotkey } from "@tanstack/hotkeys";
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
  { id: "app.settings", group: "App", label: "Settings", def: "Mod+[Comma]" },
  { id: "app.keybinds", group: "App", label: "Keyboard shortcuts", def: "Shift+[Slash]" },
];

export const keybindDef = (id: string): KeybindDef | undefined =>
  KEYBINDS.find((keybind) => keybind.id === id);

/**
 * Effective hotkey from a keybinds override map — string override wins,
 * null disables, anything else (missing or corrupt) falls back to the
 * registered default.
 */
export const resolveKeybind = (
  keybinds: Record<string, string | null>,
  id: string,
): string | null => {
  const override = keybinds[id];
  if (override === null) return null;
  if (typeof override === "string") return override;
  return keybindDef(id)?.def ?? null;
};

/** Effective hotkey — settings override wins; null means disabled. */
export const resolveKey = (settings: SepiaSettings, id: string): string | null =>
  resolveKeybind(settings.keybinds, id);

/**
 * Other actions whose effective key collides with `id`'s. The runtime fires
 * every conflicting registration (conflictBehavior "warn"), so the rebind UI
 * surfaces these as a warning instead of silently stacking handlers.
 */
export const keybindConflicts = (
  settings: SepiaSettings,
  id: string,
  platform?: "mac" | "windows" | "linux",
): KeybindDef[] => {
  const key = resolveKey(settings, id);
  if (key === null) return [];
  return KEYBINDS.filter((other) => {
    if (other.id === id) return false;
    const otherKey = resolveKey(settings, other.id);
    if (otherKey === null) return false;
    try {
      return areHotkeysEqual(key as RegisterableHotkey, otherKey as RegisterableHotkey, platform);
    } catch {
      // An unparseable stored override still conflicts on exact match.
      return key === otherKey;
    }
  });
};

/** Display tokens for a hotkey string: "Mod+K" → ["Ctrl", "K"] / ["⌘", "K"]. */
export const formatKey = (key: string, modKey: string): string[] =>
  key.split("+").map((part) => {
    if (part === "Mod") return modKey;
    // [Code] → a readable glyph: [Slash] → /, [Comma] → ,, else strip brackets.
    if (part.startsWith("[") && part.endsWith("]")) {
      const code = part.slice(1, -1);
      return code === "Slash" ? "/" : code === "Comma" ? "," : code.replace(/^Key/, "");
    }
    return part;
  });

/** Registered so a disabled binding keeps a dead registration — `enabled:false` suppresses firing. */
const DISABLED_HOTKEY = "Mod+Shift+F24";

/** useHotkey driven by the keybind registry — honors overrides + disables. */
export const useAppHotkey = (
  id: string,
  callback: Parameters<typeof useHotkey>[1],
  options?: UseHotkeyOptions,
): void => {
  const keybinds = useStore(settingsStore, (state) => state.keybinds);
  const key = resolveKeybind(keybinds, id);
  useHotkey((key ?? DISABLED_HOTKEY) as Parameters<typeof useHotkey>[0], callback, {
    ...options,
    // A caller-supplied `enabled:false` must keep suppressing the binding.
    enabled: key !== null && (options?.enabled ?? true),
    // Disabled bindings share the sentinel key — allow the collision quietly.
    ...(key === null ? { conflictBehavior: "allow" as const } : {}),
  });
};
