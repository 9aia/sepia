import { useSyncExternalStore } from "react";

/**
 * True when the device is likely to have a physical keyboard — proxied by a
 * fine pointer (mouse/trackpad). Phones report `coarse`; a keyboard-connected
 * tablet reports `fine`. Used to hide keybind affordances on touch-only UIs.
 */
const QUERY = "(pointer: fine) and (hover: hover)";

export const hasKeyboard = (): boolean =>
  typeof matchMedia !== "undefined" && matchMedia(QUERY).matches;

const subscribe = (onChange: () => void): (() => void) => {
  if (typeof matchMedia === "undefined") return () => {};
  const mql = matchMedia(QUERY);
  mql.addEventListener("change", onChange);
  return () => mql.removeEventListener("change", onChange);
};

export const useHasKeyboard = (): boolean =>
  useSyncExternalStore(subscribe, hasKeyboard, () => false);

/**
 * Input-like element where typing must not trigger app shortcuts — mirrors
 * @tanstack/hotkeys' input guard: text inputs, textarea, select, and
 * contenteditable; button-type inputs are excluded so Escape-style bindings
 * still fire when a form button has focus.
 */
export const isFormField = (el: Element | null): boolean => {
  if (el === null || typeof HTMLElement === "undefined") return false;
  if (el instanceof HTMLInputElement) {
    const type = el.type.toLowerCase();
    return type !== "button" && type !== "submit" && type !== "reset";
  }
  if (el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) return true;
  return el instanceof HTMLElement && el.isContentEditable;
};
