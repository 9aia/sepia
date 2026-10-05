// @vitest-environment happy-dom
/** DOM-side of lib/keyboard — element guards and the matchMedia hook. */
import { describe, expect, it, vi } from "vite-plus/test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { useHasKeyboard, isFormField } from "../lib/keyboard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("isFormField", () => {
  it("guards text inputs, textarea, select and contenteditable", () => {
    expect(isFormField(document.createElement("textarea"))).toBe(true);
    expect(isFormField(document.createElement("select"))).toBe(true);
    const text = document.createElement("input");
    text.type = "text";
    expect(isFormField(text)).toBe(true);
    const button = document.createElement("input");
    button.type = "button";
    expect(isFormField(button)).toBe(false);
    const div = document.createElement("div");
    expect(isFormField(div)).toBe(false);
    expect(isFormField(null)).toBe(false);
  });
});

describe("useHasKeyboard", () => {
  it("tracks the fine-pointer media query", () => {
    let listener: (() => void) | undefined;
    const mql = {
      matches: true,
      addEventListener: (_e: string, fn: () => void) => {
        listener = fn;
      },
      removeEventListener: () => {},
    };
    vi.stubGlobal("matchMedia", () => mql);

    let current: boolean | undefined;
    const Probe = () => {
      current = useHasKeyboard();
      return null;
    };
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    act(() => root.render(<Probe />));
    expect(current).toBe(true);

    mql.matches = false;
    act(() => listener?.());
    expect(current).toBe(false);
    act(() => root.unmount());
    vi.unstubAllGlobals();
  });
});
