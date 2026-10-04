import { nodeKey } from "./format";

/**
 * A node row of the "Resume on…" menu. The machine serving this UI has no
 * `node` id — its rows tag as `"local"` in merged lists — and peers carry
 * their registered id. `agents` is the roster that node reported.
 */
export interface ResumeTargetNode {
  /** Registered peer id; `undefined` is the machine serving this UI. */
  readonly node?: string;
  readonly label: string;
  readonly agents: ReadonlyArray<string>;
}

/**
 * The "Resume on…" target list, given this machine plus every registered
 * peer (`candidates`, local first) and the session being resumed. Two rules:
 *
 * - Single machine: with no peers the only candidate is the local node, and
 *   resuming where the session already lives is a convert, not this action —
 *   the menu hides (empty list).
 * - The session's own node+agent pair is excluded from that node's roster:
 *   "Resume on thinkpad · devin" for a `devin@thinkpad` session is a no-op.
 *   A peer-hosted session still sees this machine plus the other peers —
 *   only its own pair drops out.
 *
 * `current` may be undefined (no session selected): the single-node gate
 * still applies, rosters pass through unfiltered.
 */
export const resumeTargets = (
  candidates: ReadonlyArray<ResumeTargetNode>,
  current: { readonly node?: string; readonly agent: string } | undefined,
): ResumeTargetNode[] => {
  if (candidates.length <= 1) return [];
  return candidates.map((entry) => ({
    ...entry,
    agents:
      current === undefined
        ? entry.agents
        : entry.agents.filter(
            (id) => !(nodeKey(entry.node) === nodeKey(current.node) && id === current.agent),
          ),
  }));
};
