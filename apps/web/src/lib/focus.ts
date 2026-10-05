import { useStore } from "@tanstack/react-store";
import { LOCAL_NODE_ID, nodeKey } from "./format";
import {
  desktopCwdFor,
  onDesktop,
  recentCwdFor,
  setDesktop,
  settingsStore,
  type DesktopEnvironment,
  type SepiaSettings,
} from "./settings";

/**
 * The desktop model: `settings.desktop` is the client's current working
 * environment — which machine, agent, model and directory "New session"
 * aims at. It replaced both the ephemeral `sepiaStore.focus` and the
 * per-node creation defaults (`defaultAgent`/`defaultCwd` maps): there's
 * one concept now, persisted — the footer's focus picks and Settings →
 * Desktop write the same four fields, and an unset field falls back
 * (node → this machine, agent → the node's pick, model → the agent's
 * configured pref, cwd → the node's most recent session dir).
 */

/** The desktop as reactive state — same pattern as `useStore(sepiaStore, …)`. */
export const useFocus = (): DesktopEnvironment => useStore(settingsStore, (state) => state.desktop);

/**
 * Write a desktop pick (the footer's machine+agent menu, Settings →
 * Desktop's fields). `patch` merges — a machine change should clear the
 * scoped dims (`agent`/`model`/`cwd`) since they don't port across nodes.
 */
export const setFocus = (patch: Partial<DesktopEnvironment>): void => setDesktop(patch);

/** Back to the unscoped desktop — this machine, node pick, recents. */
export const clearFocus = (): void =>
  setDesktop({ node: null, agent: null, model: null, cwd: null });

/**
 * The `node` value API call sites want: `desktop.node` is a nodeKey (or
 * null), so a local desktop reads "local" — normalized to `undefined` here
 * since API targets treat undefined/"local" identically.
 */
export const focusNode = (desktop: DesktopEnvironment): string | undefined => {
  const key = nodeKey(desktop.node ?? undefined);
  return key === LOCAL_NODE_ID ? undefined : key;
};

/**
 * The node+agent+model a create targets. An explicit `node` argument (a
 * folder row's "New session here" naming its own machine) always wins —
 * the desktop's agent/model then only apply when that node IS the
 * desktop's (a pick scoped to one machine never leaks onto another).
 * Unset results mean "the node picks" — callers may still add the local
 * "first agent" fallback.
 */
export const resolveCreateTarget = (
  settings: SepiaSettings,
  explicitNode: string | undefined,
): { node: string | undefined; agent: string | null; model: string | null } => {
  const desktop = settings.desktop;
  if (explicitNode !== undefined) {
    const scoped = onDesktop(settings, explicitNode);
    return {
      node: explicitNode,
      agent: scoped ? desktop.agent : null,
      model: scoped ? desktop.model : null,
    };
  }
  return { node: focusNode(desktop), agent: desktop.agent, model: desktop.model };
};

export interface CreateCwdInput {
  /** The sidebar's picked cwd — a local-machine dir only. */
  readonly picked: string | null;
  readonly sessions: ReadonlyArray<{ readonly cwd: string; readonly node?: string }>;
  readonly homedir: string | undefined;
}

/**
 * The spawn dir for a default create under the desktop. A peer desktop
 * resolves directories on that node — the picked cwd and homedir are local
 * paths that wouldn't exist there — so the chain shortens to the desktop's
 * dir pick, then that node's most recent session dir.
 */
export const resolveCreateCwd = (settings: SepiaSettings, input: CreateCwdInput): string => {
  const node = focusNode(settings.desktop);
  if (node !== undefined) {
    return desktopCwdFor(settings, node) ?? recentCwdFor(input.sessions, node) ?? "/";
  }
  return (
    input.picked ??
    settings.desktop.cwd ??
    recentCwdFor(input.sessions, undefined) ??
    input.homedir ??
    "/"
  );
};
