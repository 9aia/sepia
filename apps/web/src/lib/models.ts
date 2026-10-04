import type { SepiaSettings } from "./settings";

/**
 * Resolves spawn-time model args for a session's agent: the session's own
 * model preference wins, then the agent's configured default. Fallbacks only
 * apply in "auto" mode.
 */
export const modelArgsFor = (
  agentId: string,
  sessionModel: string | null | undefined,
  settings: SepiaSettings,
): { readonly model?: string; readonly fallbacks?: ReadonlyArray<string> } => {
  const pref = settings.models[agentId];
  const configured = pref?.model.trim() !== "" ? pref?.model : undefined;
  const model = sessionModel ?? configured;
  const fallbacks =
    pref?.mode === "auto"
      ? pref.fallbacks
          .split(",")
          .map((f) => f.trim())
          .filter((f) => f !== "")
      : undefined;
  return {
    model,
    fallbacks: fallbacks !== undefined && fallbacks.length > 0 ? fallbacks : undefined,
  };
};
