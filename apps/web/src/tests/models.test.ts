import { describe, expect, it } from "vite-plus/test";
import { modelArgsFor } from "../lib/models";
import type { SepiaSettings } from "../lib/settings";
import { defaultSidebarSections } from "../lib/sidebar";

const settings = (
  models: SepiaSettings["models"],
  desktop: Partial<SepiaSettings["desktop"]> = {},
): SepiaSettings => ({
  desktop: { node: null, agent: null, model: null, cwd: null, ...desktop },
  localNodeName: null,
  localNodeUrl: null,
  localNodeEnabled: true,
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

  it("applies the desktop's model when the session's agent matches", () => {
    const s = settings({}, { agent: "devin", model: "claude-x" });
    // Local desktop → sessions with no node field are on its machine.
    expect(modelArgsFor("devin", null, s).model).toBe("claude-x");
    expect(modelArgsFor("devin", null, s, "local").model).toBe("claude-x");
    expect(modelArgsFor("devin", null, s, undefined).model).toBe("claude-x");
    // A different agent or node doesn't inherit the pick.
    expect(modelArgsFor("cline", null, s).model).toBeUndefined();
    expect(modelArgsFor("devin", null, s, "node_a1b2").model).toBeUndefined();
    // A peer-scoped desktop applies on its own node only.
    const peer = settings({}, { node: "node_a1b2", agent: "devin", model: "gpt-y" });
    expect(modelArgsFor("devin", null, peer, "node_a1b2").model).toBe("gpt-y");
    expect(modelArgsFor("devin", null, peer).model).toBeUndefined();
  });

  it("the session's own model still beats the desktop pick, which beats the agent pref", () => {
    const s = settings(
      { devin: { model: "configured", fallbacks: "", mode: "manual" } },
      { agent: "devin", model: "desktop-m" },
    );
    expect(modelArgsFor("devin", "session-m", s).model).toBe("session-m");
    expect(modelArgsFor("devin", null, s).model).toBe("desktop-m");
    // Off the desktop's agent, the configured pref resumes.
    expect(modelArgsFor("cline", null, s).model).toBeUndefined();
  });

  it("a desktop with no agent pick applies its model to whatever runs", () => {
    const s = settings({}, { model: "m1" });
    expect(modelArgsFor("devin", null, s).model).toBe("m1");
    expect(modelArgsFor("cline", null, s).model).toBe("m1");
  });
});
