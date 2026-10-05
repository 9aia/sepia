import { useStore } from "@tanstack/react-store";
import { LOCAL_NODE_ID } from "./format";
import { defaultAgentFor, defaultCwdFor, recentCwdFor, type SepiaSettings } from "./settings";
import { sepiaStore, type FocusTarget } from "./store";

/**
 * The focus model: `sepiaStore.focus` is the sidebar's explicit create
 * target — which machine+agent "New session" aims at. It overrides the
 * per-node settings defaults (a focus is an explicit pick); a null focus
 * leaves the default chain in charge. Focus lives in the ephemeral UI
 * store, not settings — it's a "right now" aim, not a preference.
 */

/** The focused target as reactive state — same pattern as `useStore(sepiaStore, …)`. */
export const useFocus = (): FocusTarget | null => useStore(sepiaStore, (state) => state.focus);

/**
 * The `node` value call sites want: `FocusTarget.node` is a nodeKey, so a
 * local focus reads "local" — normalized to `undefined` here since API
 * targets treat undefined/"local" identically.
 */
export const focusNode = (focus: FocusTarget | null): string | undefined =>
  focus === null || focus.node === LOCAL_NODE_ID ? undefined : focus.node;

/**
 * The node+agent a create targets. An explicit `node` argument (a folder
 * row's "New session here" naming its own machine) always wins — focus
 * only drives creates that didn't pick a node. Agent precedence for the
 * focused node: `focus.agent` > the node's configured default > null (the
 * node picks; callers may still add the local "first agent" fallback).
 */
export const resolveCreateTarget = (
  focus: FocusTarget | null,
  settings: SepiaSettings,
  explicitNode: string | undefined,
): { node: string | undefined; agent: string | null } => {
  if (explicitNode !== undefined) {
    return { node: explicitNode, agent: defaultAgentFor(settings, explicitNode) };
  }
  const node = focusNode(focus);
  return { node, agent: focus?.agent ?? defaultAgentFor(settings, node) };
};

export interface CreateCwdInput {
  /** The sidebar's picked cwd — a local-machine dir only. */
  readonly picked: string | null;
  readonly settings: SepiaSettings;
  readonly sessions: ReadonlyArray<{ readonly cwd: string; readonly node?: string }>;
  readonly homedir: string | undefined;
}

/**
 * The spawn dir for a default create under `focus`. A peer focus resolves
 * directories on that node — the picked cwd and homedir are local paths
 * that wouldn't exist there — so the chain shortens to the peer's
 * configured default, then its most recent session's dir.
 */
export const resolveCreateCwd = (focus: FocusTarget | null, input: CreateCwdInput): string => {
  const node = focusNode(focus);
  if (node !== undefined) {
    return defaultCwdFor(input.settings, node) ?? recentCwdFor(input.sessions, node) ?? "/";
  }
  return (
    input.picked ??
    defaultCwdFor(input.settings, undefined) ??
    recentCwdFor(input.sessions, undefined) ??
    input.homedir ??
    "/"
  );
};
