import { describe, expect, it } from "vite-plus/test";
import { formatKey, KEYBINDS, keybindDef, resolveKey } from "../lib/keybinds";
import type { SepiaSettings } from "../lib/settings";

const settings = (keybinds: Record<string, string | null>): SepiaSettings => ({
  defaultAgent: null,
  defaultCwd: null,
  models: {},
  keybinds,
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
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
