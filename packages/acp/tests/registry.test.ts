import { expect, test } from "vite-plus/test";
import { builtinAgents, resolveAgent } from "../src/AgentRegistry.js";
import type { AgentSpec } from "../src/types.js";

test("exposes the builtin agents", () => {
  expect(builtinAgents.map((agent) => agent.id)).toEqual(["devin", "cline"]);
  expect(resolveAgent("devin")).toEqual({ id: "devin", label: "Devin", command: ["devin", "acp"] });
  expect(resolveAgent("cline")).toEqual({
    id: "cline",
    label: "Cline",
    command: ["cline", "--acp"],
  });
});

test("overrides win by id", () => {
  const override: AgentSpec = { id: "devin", label: "Custom", command: ["custom", "acp"] };
  expect(resolveAgent("devin", [override])).toBe(override);
});

test("an unknown id throws", () => {
  expect(() => resolveAgent("nope")).toThrow(/Unknown agent "nope"/);
});
