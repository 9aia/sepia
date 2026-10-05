import { isModelEnabled } from "./catalog";
import { desktopModelFor, type SepiaSettings } from "./settings";

/**
 * Resolves spawn-time model args for a session's agent: the session's own
 * model preference wins, then the desktop's model pick (when the session's
 * node+agent are the desktop's — `node` defaults to this machine), then the
 * agent's configured default. Fallbacks only apply in "auto" mode.
 *
 * Parked models (`settings.disabledModels` — `node:agent:model` keys) are
 * never emitted: each candidate is checked against the resolved node+agent
 * pair and the first enabled one wins, so a stored-but-parked session or
 * desktop pick silently falls through to the next source rather than
 * spawning a model the user took off the menu. Parked fallbacks drop out
 * of the list individually.
 */
export const modelArgsFor = (
  agentId: string,
  sessionModel: string | null | undefined,
  settings: SepiaSettings,
  node?: string,
): { readonly model?: string; readonly fallbacks?: ReadonlyArray<string> } => {
  const pref = settings.models[agentId];
  const configured = pref?.model.trim() !== "" ? pref?.model : undefined;
  const model = [sessionModel, desktopModelFor(settings, node, agentId), configured].find(
    (candidate): candidate is string =>
      candidate !== null &&
      candidate !== undefined &&
      candidate !== "" &&
      isModelEnabled(settings, node, agentId, candidate),
  );
  const fallbacks =
    pref?.mode === "auto"
      ? pref.fallbacks
          .split(",")
          .map((f) => f.trim())
          .filter((f) => f !== "" && isModelEnabled(settings, node, agentId, f))
      : undefined;
  return {
    model,
    fallbacks: fallbacks !== undefined && fallbacks.length > 0 ? fallbacks : undefined,
  };
};
