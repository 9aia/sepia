import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { hasKeyboard } from "../lib/keyboard";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("hasKeyboard", () => {
  it("is false when matchMedia is unavailable", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(hasKeyboard()).toBe(false);
  });

  it("mirrors the fine-pointer media query", () => {
    const query = vi.fn(() => ({ matches: true }));
    vi.stubGlobal("matchMedia", query);
    expect(hasKeyboard()).toBe(true);
    expect(query).toHaveBeenCalledWith("(pointer: fine) and (hover: hover)");
  });

  it("is false on coarse-pointer devices", () => {
    vi.stubGlobal("matchMedia", () => ({ matches: false }));
    expect(hasKeyboard()).toBe(false);
  });
});
