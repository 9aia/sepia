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
