import type { AgentSpec } from "./types.js";

export const builtinAgents: ReadonlyArray<AgentSpec> = [
  { id: "devin", label: "Devin", command: ["devin", "acp"] },
  { id: "cline", label: "Cline", command: ["cline", "--acp"] },
];

export const resolveAgent = (id: string, overrides: ReadonlyArray<AgentSpec> = []): AgentSpec => {
  const override = overrides.find((agent) => agent.id === id);
  if (override !== undefined) return override;

  const builtin = builtinAgents.find((agent) => agent.id === id);
  if (builtin !== undefined) return builtin;

  const known = [...overrides, ...builtinAgents].map((agent) => agent.id).join(", ");
  throw new Error(`Unknown agent "${id}"${known === "" ? "" : `; known agents: ${known}`}`);
};
