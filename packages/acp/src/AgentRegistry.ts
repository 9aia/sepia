import type { AgentSpec } from "./types.js";

export const builtinAgents: ReadonlyArray<AgentSpec> = [
  { id: "devin", label: "Devin", command: ["devin", "acp"] },
  { id: "cline", label: "Cline", command: ["cline", "--acp"] },
  // Claude Code has no native ACP mode; the agentclientprotocol adapter
  // (`npm i -g @agentclientprotocol/claude-agent-acp`) bridges it and
  // supports session/load + session/list over the JSONL transcripts.
  { id: "claude", label: "Claude Code", command: ["claude-agent-acp"] },
];

export const resolveAgent = (id: string, overrides: ReadonlyArray<AgentSpec> = []): AgentSpec => {
  const override = overrides.find((agent) => agent.id === id);
  if (override !== undefined) return override;

  const builtin = builtinAgents.find((agent) => agent.id === id);
  if (builtin !== undefined) return builtin;

  const known = [...overrides, ...builtinAgents].map((agent) => agent.id).join(", ");
  throw new Error(`Unknown agent "${id}"${known === "" ? "" : `; known agents: ${known}`}`);
};

/**
 * Per-agent spawn flags for model selection — appended to the agent's
 * command at spawn time. devin takes fuzzy names + an ordered refusal
 * fallback list; cline takes a single `-m` model id; claude's ACP adapter
 * picks the model inside the session, not on argv.
 */
export const modelArgs = (
  agentId: string,
  model: string | undefined,
  fallbacks: ReadonlyArray<string> | undefined,
): ReadonlyArray<string> => {
  const args: Array<string> = [];
  if (agentId === "cline") {
    if (model !== undefined) args.push("-m", model);
    return args;
  }
  if (agentId === "claude") return args;
  if (model !== undefined) args.push("--model", model);
  if (agentId === "devin" && fallbacks !== undefined && fallbacks.length > 0) {
    args.push("--refusal-fallback", fallbacks.join(","));
  }
  return args;
};
