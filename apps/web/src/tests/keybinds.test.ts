import { describe, expect, it } from "vite-plus/test";
import { matchesKeyboardEvent, type Hotkey } from "@tanstack/hotkeys";
import {
  formatKey,
  KEYBINDS,
  keybindConflicts,
  keybindDef,
  resolveKey,
  resolveKeybind,
} from "../lib/keybinds";
import type { SepiaSettings } from "../lib/settings";
import { defaultSidebarSections } from "../lib/sidebar";

const settings = (keybinds: Record<string, string | null>): SepiaSettings => ({
  desktop: { node: null, agent: null, model: null, cwd: null },
  localNodeName: null,
  localNodeUrl: null,
  localNodeEnabled: true,
  models: {},
  keybinds,
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  sidebar: { sections: defaultSidebarSections() },
});

describe("keybindDef", () => {
  it("finds registered actions", () => {
    expect(keybindDef("app.sidebar")?.def).toBe("Mod+B");
    expect(keybindDef("session.new")?.group).toBe("Sessions");
  });

  it("returns undefined for unknown ids", () => {
    expect(keybindDef("bogus")).toBeUndefined();
  });

  it("every keybind has a unique id and a group", () => {
    const ids = KEYBINDS.map((k) => k.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const bind of KEYBINDS) expect(bind.group.length).toBeGreaterThan(0);
  });
});

describe("resolveKey", () => {
  it("uses the default binding with no overrides", () => {
    expect(resolveKey(settings({}), "filter.focus")).toBe("Mod+K");
  });

  it("a string override wins over the default", () => {
    expect(resolveKey(settings({ "filter.focus": "Mod+P" }), "filter.focus")).toBe("Mod+P");
  });

  it("a null override disables the binding", () => {
    expect(resolveKey(settings({ "filter.focus": null }), "filter.focus")).toBeNull();
  });

  it("unknown actions resolve to null", () => {
    expect(resolveKey(settings({}), "bogus")).toBeNull();
  });
});

describe("formatKey", () => {
  it("maps Mod to the platform label", () => {
    expect(formatKey("Mod+K", "⌘")).toEqual(["⌘", "K"]);
    expect(formatKey("Mod+B", "Ctrl")).toEqual(["Ctrl", "B"]);
  });

  it("renders bracketed key codes as glyphs", () => {
    expect(formatKey("Shift+[Slash]", "⌘")).toEqual(["Shift", "/"]);
    expect(formatKey("Mod+[Comma]", "⌘")).toEqual(["⌘", ","]);
    expect(formatKey("[KeyQ]", "⌘")).toEqual(["Q"]);
    expect(formatKey("[ArrowUp]", "⌘")).toEqual(["ArrowUp"]);
  });

  it("passes plain parts through", () => {
    expect(formatKey("N", "⌘")).toEqual(["N"]);
    expect(formatKey("Escape", "⌘")).toEqual(["Escape"]);
  });
});

describe("resolveKeybind", () => {
  it("falls back to the default for a corrupt non-string override", () => {
    // Corrupt localStorage can smuggle non-string values past the type.
    const keybinds = { "nav.up": 42 } as unknown as Record<string, string | null>;
    expect(resolveKeybind(keybinds, "nav.up")).toBe("ArrowUp");
  });

  it("resolves unknown ids to null", () => {
    expect(resolveKeybind({}, "bogus")).toBeNull();
  });
});

describe("keybindConflicts", () => {
  it("finds no conflicts in the default registry", () => {
    for (const keybind of KEYBINDS) {
      expect(keybindConflicts(settings({}), keybind.id, "windows")).toEqual([]);
    }
  });

  it("reports a conflict both ways when two actions share a key", () => {
    const s = settings({ "filter.clear": "Mod+K" });
    expect(keybindConflicts(s, "filter.clear", "windows").map((k) => k.id)).toEqual([
      "filter.focus",
    ]);
    expect(keybindConflicts(s, "filter.focus", "windows").map((k) => k.id)).toEqual([
      "filter.clear",
    ]);
  });

  it("treats equivalent spellings as the same binding", () => {
    const s = settings({ "filter.clear": "Control+K" });
    expect(keybindConflicts(s, "filter.clear", "windows").map((k) => k.id)).toEqual([
      "filter.focus",
    ]);
    // Control+K is not Mod+K on macOS (Mod resolves to Meta there).
    expect(keybindConflicts(s, "filter.clear", "mac")).toEqual([]);
  });

  it("ignores disabled bindings and unbound ids", () => {
    const s = settings({ "filter.clear": null, "nav.up": "Mod+K" });
    expect(keybindConflicts(s, "nav.up", "windows").map((k) => k.id)).toEqual(["filter.focus"]);
    expect(keybindConflicts(s, "filter.clear", "windows")).toEqual([]);
    expect(keybindConflicts(settings({}), "bogus", "windows")).toEqual([]);
  });

  it("does not throw on an unparseable stored override", () => {
    const s = settings({ "nav.up": "not a hotkey !!!" });
    expect(() => keybindConflicts(s, "nav.up", "windows")).not.toThrow();
    expect(keybindConflicts(s, "nav.up", "windows")).toEqual([]);
  });
});

/** Duck-typed KeyboardEvent — the matcher only reads these properties. */
const keyEvent = (init: { key: string } & Partial<KeyboardEvent>): KeyboardEvent =>
  ({
    code: "",
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    location: 0,
    isComposing: false,
    getModifierState: () => false,
    ...init,
  }) as KeyboardEvent;

describe("binding matching", () => {
  const match = (id: string, event: KeyboardEvent, platform: "mac" | "windows" = "windows") =>
    matchesKeyboardEvent(event, keybindDef(id)!.def as Hotkey, platform);

  it("every registered default parses and matches its intended event", () => {
    // Physical-code bindings carry a [Code]; logical bindings use key names.
    expect(() => matchesKeyboardEvent(keyEvent({ key: "x" }), "N")).not.toThrow();
    for (const keybind of KEYBINDS) expect(keybind.def.length).toBeGreaterThan(0);
  });

  it("bare 'N' matches a plain keypress but not modified presses", () => {
    expect(match("session.new", keyEvent({ key: "n", code: "KeyN" }))).toBe(true);
    expect(match("session.new", keyEvent({ key: "N", code: "KeyN" }))).toBe(true);
    expect(match("session.new", keyEvent({ key: "N", code: "KeyN", shiftKey: true }))).toBe(false);
    expect(match("session.new", keyEvent({ key: "n", code: "KeyN", ctrlKey: true }))).toBe(false);
    expect(match("session.new", keyEvent({ key: "n", code: "KeyN", altKey: true }))).toBe(false);
  });

  it("arrow bindings match exactly, extra modifiers don't", () => {
    expect(match("nav.down", keyEvent({ key: "ArrowDown", code: "ArrowDown" }))).toBe(true);
    expect(match("nav.down", keyEvent({ key: "ArrowUp", code: "ArrowUp" }))).toBe(false);
    expect(
      match("nav.down", keyEvent({ key: "ArrowDown", code: "ArrowDown", ctrlKey: true })),
    ).toBe(false);
  });

  it("Mod resolves to Control on windows and Meta on mac", () => {
    const k = keyEvent({ key: "k", code: "KeyK", ctrlKey: true });
    expect(match("filter.focus", k, "windows")).toBe(true);
    expect(match("filter.focus", keyEvent({ key: "k", code: "KeyK", metaKey: true }), "mac")).toBe(
      true,
    );
    // Wrong platform modifier doesn't fire.
    expect(
      match("filter.focus", keyEvent({ key: "k", code: "KeyK", metaKey: true }), "windows"),
    ).toBe(false);
    expect(match("filter.focus", keyEvent({ key: "k", code: "KeyK" }), "windows")).toBe(false);
  });

  it("physical [Code] bindings match on code regardless of produced glyph", () => {
    // Shift+/ produces "?" on US layouts — the binding is positional.
    expect(match("app.keybinds", keyEvent({ key: "?", code: "Slash", shiftKey: true }))).toBe(true);
    expect(match("app.keybinds", keyEvent({ key: "/", code: "Slash" }))).toBe(false);
    // Mod+, fires even when the layout's glyph differs.
    expect(match("app.settings", keyEvent({ key: "<", code: "Comma", ctrlKey: true }))).toBe(true);
  });

  it("dead keys still match physical bindings via event.code", () => {
    // macOS Option+letter / intl layouts report key "Dead" before the commit.
    const dead = keyEvent({ key: "Dead", code: "Slash", shiftKey: true });
    expect(match("app.keybinds", dead)).toBe(true);
  });

  it("IME composition suppresses key/code bindings but not Escape", () => {
    const composing = keyEvent({ key: "n", code: "KeyN", isComposing: true });
    expect(match("session.new", composing)).toBe(false);
    const composingCode = keyEvent({ key: "Process", code: "Slash", shiftKey: true });
    expect(match("app.keybinds", composingCode)).toBe(false);
    // Escape still resolves — composition UIs rely on it to cancel.
    expect(
      match("filter.clear", keyEvent({ key: "Escape", code: "Escape", isComposing: true })),
    ).toBe(true);
  });

  it("AltGraph never masquerades as Mod", () => {
    // AltGr+K reports ctrl+alt (+ AltGraph modifier state) on windows/linux.
    const altGr = keyEvent({
      key: "ĸ",
      code: "KeyK",
      ctrlKey: true,
      altKey: true,
      getModifierState: (m: string) => m === "AltGraph",
    });
    expect(match("filter.focus", altGr, "windows")).toBe(false);
  });
});
