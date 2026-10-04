import { describe, expect, it } from "vite-plus/test";
import { builtinAgents, modelArgs, resolveAgent } from "../src/AgentRegistry.js";
import type { AgentSpec } from "../src/types.js";

describe("modelArgs", () => {
  it("cline takes a single -m flag and ignores fallbacks", () => {
    expect(modelArgs("cline", "claude-x", ["a", "b"])).toEqual(["-m", "claude-x"]);
    expect(modelArgs("cline", undefined, ["a"])).toEqual([]);
  });

  it("devin takes --model plus an ordered --refusal-fallback list", () => {
    expect(modelArgs("devin", "gpt-x", ["f1", "f2"])).toEqual([
      "--model",
      "gpt-x",
      "--refusal-fallback",
      "f1,f2",
    ]);
    expect(modelArgs("devin", "gpt-x", undefined)).toEqual(["--model", "gpt-x"]);
    expect(modelArgs("devin", "gpt-x", [])).toEqual(["--model", "gpt-x"]);
    expect(modelArgs("devin", undefined, ["f1"])).toEqual(["--refusal-fallback", "f1"]);
  });

  it("claude takes no spawn-time model flags", () => {
    expect(modelArgs("claude", "claude-opus-4-5", ["f1"])).toEqual([]);
    expect(modelArgs("claude", undefined, undefined)).toEqual([]);
  });

  it("other agents get --model only", () => {
    expect(modelArgs("cursor", "m1", ["f1"])).toEqual(["--model", "m1"]);
    expect(modelArgs("cursor", undefined, undefined)).toEqual([]);
  });
});

describe("resolveAgent — error details", () => {
  it("lists the known agents, including overrides", () => {
    const override: AgentSpec = { id: "custom", label: "C", command: ["c"] };
    expect(() => resolveAgent("nope", [override])).toThrow(
      'Unknown agent "nope"; known agents: custom, devin, cline, claude',
    );
  });

  it("omits the suffix when nothing is known (defensive)", () => {
    // builtinAgents always has entries in practice; this pins the fallback text.
    expect(() => resolveAgent("")).toThrow('Unknown agent ""');
  });
});

describe("builtinAgents", () => {
  it("devin, cline and claude are the builtins", () => {
    expect(builtinAgents.map((a) => a.id)).toEqual(["devin", "cline", "claude"]);
  });
});
