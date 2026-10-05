import { describe, expect, it } from "vite-plus/test";
import { modelArgsFor } from "../lib/models";
import type { SepiaSettings } from "../lib/settings";
import { defaultSidebarSections } from "../lib/sidebar";

const settings = (models: SepiaSettings["models"]): SepiaSettings => ({
  defaultAgent: {},
  defaultCwd: {},
  localNodeName: null,
  models,
  keybinds: {},
  notifications: { enabled: false, done: true, permission: true },
  theme: "dark",
  sidebar: { sections: defaultSidebarSections() },
});

describe("modelArgsFor", () => {
  it("returns nothing with no preference and no session model", () => {
    expect(modelArgsFor("devin", null, settings({}))).toEqual({
      model: undefined,
      fallbacks: undefined,
    });
  });

  it("prefers the session's own model over the configured default", () => {
    const s = settings({ devin: { model: "claude-x", fallbacks: "", mode: "manual" } });
    expect(modelArgsFor("devin", "gpt-y", s)).toEqual({
      model: "gpt-y",
      fallbacks: undefined,
    });
  });

  it("falls back to the configured model when the session has none", () => {
    const s = settings({ devin: { model: "claude-x", fallbacks: "", mode: "manual" } });
    expect(modelArgsFor("devin", undefined, s)).toEqual({
      model: "claude-x",
      fallbacks: undefined,
    });
    expect(modelArgsFor("devin", null, s).model).toBe("claude-x");
  });

  it("treats a blank configured model as unset", () => {
    const s = settings({ devin: { model: "  ", fallbacks: "a", mode: "auto" } });
    expect(modelArgsFor("devin", null, s).model).toBeUndefined();
  });

  it("auto mode parses the comma-separated fallback list", () => {
    const s = settings({
      devin: { model: "m1", fallbacks: " m2 , m3 ,, ", mode: "auto" },
    });
    expect(modelArgsFor("devin", null, s)).toEqual({
      model: "m1",
      fallbacks: ["m2", "m3"],
    });
  });

  it("manual mode never emits fallbacks", () => {
    const s = settings({ devin: { model: "m1", fallbacks: "m2,m3", mode: "manual" } });
    expect(modelArgsFor("devin", null, s).fallbacks).toBeUndefined();
  });

  it("an all-empty fallback list collapses to undefined", () => {
    const s = settings({ devin: { model: "m1", fallbacks: " , ,", mode: "auto" } });
    expect(modelArgsFor("devin", null, s).fallbacks).toBeUndefined();
  });
});
