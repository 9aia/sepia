// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { detectPlatform } from "@tanstack/hotkeys";
import type { UseHotkeyOptions } from "@tanstack/react-hotkeys";
import { useAppHotkey } from "../lib/keybinds";
import { isFormField } from "../lib/keyboard";
import { setSettings } from "../lib/settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
const mounted: HTMLElement[] = [];

interface HarnessProps {
  readonly id: string;
  readonly cb: () => void;
  readonly options?: UseHotkeyOptions;
}

const Harness = ({ id, cb, options }: HarnessProps): null => {
  useAppHotkey(id, cb, options);
  return null;
};

const mount = (props: HarnessProps): Root => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  mounted.push(host);
  const root = createRoot(host);
  roots.push(root);
  act(() => root.render(createElement(Harness, props)));
  return root;
};

/** Platform-adaptive modifier flags matching what `Mod` resolves to here. */
const MOD: KeyboardEventInit = detectPlatform() === "mac" ? { metaKey: true } : { ctrlKey: true };
const ANTI_MOD: KeyboardEventInit =
  detectPlatform() === "mac" ? { ctrlKey: true } : { metaKey: true };

const press = (init: KeyboardEventInit, target: EventTarget = document): KeyboardEvent => {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
};

const focusIn = (el: HTMLElement): void => {
  document.body.appendChild(el);
  mounted.push(el);
  el.focus();
};

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  for (const root of roots.splice(0)) act(() => root.unmount());
  for (const el of mounted.splice(0)) el.remove();
  (document.activeElement as HTMLElement | null)?.blur?.();
  act(() => setSettings({ keybinds: {} }));
  vi.restoreAllMocks();
});

describe("useAppHotkey", () => {
  it("fires the callback on its default binding", () => {
    const cb = vi.fn();
    mount({ id: "app.sidebar", cb });
    press({ key: "b", code: "KeyB", ...MOD });
    expect(cb).toHaveBeenCalledTimes(1);
    // Non-Mod modifier does not fire.
    press({ key: "b", code: "KeyB", ...ANTI_MOD });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("rebinds live when a settings override lands", () => {
    const cb = vi.fn();
    mount({ id: "app.sidebar", cb });
    act(() => setSettings({ keybinds: { "app.sidebar": "Mod+P" } }));
    press({ key: "b", code: "KeyB", ...MOD });
    expect(cb).not.toHaveBeenCalled();
    press({ key: "p", code: "KeyP", ...MOD });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("a null override disables the binding", () => {
    const cb = vi.fn();
    act(() => setSettings({ keybinds: { "app.sidebar": null } }));
    mount({ id: "app.sidebar", cb });
    press({ key: "b", code: "KeyB", ...MOD });
    expect(cb).not.toHaveBeenCalled();
  });

  it("does not warn when several bindings are disabled at once", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    act(() => setSettings({ keybinds: { "nav.up": null, "nav.down": null } }));
    mount({ id: "nav.up", cb: vi.fn() });
    mount({ id: "nav.down", cb: vi.fn() });
    expect(warn).not.toHaveBeenCalled();
  });

  it("respects a caller-supplied enabled:false", () => {
    const cb = vi.fn();
    mount({ id: "app.sidebar", cb, options: { enabled: false } });
    press({ key: "b", code: "KeyB", ...MOD });
    expect(cb).not.toHaveBeenCalled();
  });

  it("does not throw — and falls back to the default — on a corrupt override", () => {
    const cb = vi.fn();
    act(() =>
      setSettings({ keybinds: { "app.sidebar": 7 } as unknown as Record<string, string | null> }),
    );
    expect(() => mount({ id: "app.sidebar", cb })).not.toThrow();
    press({ key: "b", code: "KeyB", ...MOD });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("suppresses bare-letter bindings while a form field has focus", () => {
    const cb = vi.fn();
    const input = document.createElement("input");
    focusIn(input);
    mount({ id: "session.new", cb });
    press({ key: "n", code: "KeyN" }, input);
    expect(cb).not.toHaveBeenCalled();
    input.blur();
    press({ key: "n", code: "KeyN" });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("does not open the keybinds dialog while '?' is typed into an input", () => {
    const cb = vi.fn();
    const input = document.createElement("input");
    focusIn(input);
    mount({ id: "app.keybinds", cb });
    press({ key: "?", code: "Slash", shiftKey: true }, input);
    expect(cb).not.toHaveBeenCalled();
    input.blur();
    press({ key: "?", code: "Slash", shiftKey: true });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("Escape still fires from inside a form field", () => {
    const cb = vi.fn();
    const input = document.createElement("input");
    focusIn(input);
    mount({ id: "filter.clear", cb });
    press({ key: "Escape", code: "Escape" }, input);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("scopes firing to its target element", () => {
    const cb = vi.fn();
    const scope = document.createElement("div");
    document.body.appendChild(scope);
    mounted.push(scope);
    mount({ id: "filter.clear", cb, options: { target: scope } });
    // Outside the scope: ignored.
    press({ key: "Escape", code: "Escape" });
    expect(cb).not.toHaveBeenCalled();
    // Inside the scope: fires.
    press({ key: "Escape", code: "Escape" }, scope);
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("ignores IME composition events", () => {
    const cb = vi.fn();
    mount({ id: "session.new", cb });
    press({ key: "Process", code: "KeyN" });
    expect(cb).not.toHaveBeenCalled();
    const composing = new KeyboardEvent("keydown", {
      key: "n",
      code: "KeyN",
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(composing, "isComposing", { value: true });
    act(() => {
      document.dispatchEvent(composing);
    });
    expect(cb).not.toHaveBeenCalled();
  });

  it("fires every conflicting registration — why the dialog warns", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    act(() => setSettings({ keybinds: { "nav.up": "Mod+J", "nav.down": "Mod+J" } }));
    const up = vi.fn();
    const down = vi.fn();
    mount({ id: "nav.up", cb: up });
    mount({ id: "nav.down", cb: down });
    press({ key: "j", code: "KeyJ", ...MOD });
    expect(up).toHaveBeenCalledTimes(1);
    expect(down).toHaveBeenCalledTimes(1);
    expect(warn).toHaveBeenCalled();
  });

  it("survives a reload: a stored override is picked up from localStorage", async () => {
    act(() => setSettings({ keybinds: { "app.sidebar": "Mod+P", "nav.up": null } }));
    // Simulate reload — fresh module re-runs load() against localStorage.
    vi.resetModules();
    const mod = await import("../lib/settings");
    expect(mod.settingsStore.state.keybinds).toEqual({ "app.sidebar": "Mod+P", "nav.up": null });
  });
});

describe("isFormField", () => {
  it("flags text-entry elements", () => {
    expect(isFormField(document.createElement("input"))).toBe(true);
    expect(isFormField(document.createElement("textarea"))).toBe(true);
    expect(isFormField(document.createElement("select"))).toBe(true);
    const editable = document.createElement("div");
    editable.contentEditable = "true";
    expect(isFormField(editable)).toBe(true);
  });

  it("excludes button-type inputs and plain elements", () => {
    const button = document.createElement("input");
    button.type = "button";
    expect(isFormField(button)).toBe(false);
    expect(isFormField(document.createElement("div"))).toBe(false);
    expect(isFormField(null)).toBe(false);
  });
});
