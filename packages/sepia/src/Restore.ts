import { isAbsolute, resolve, sep } from "node:path";
import type { Session, ToolCallDiff } from "./Domain.js";

/**
 * Restore planning — pure math over a session's recorded `ToolCall.diffs`,
 * separated from the filesystem/git side effects so the same code is unit
 * testable and the control plane can inject its own exec seam.
 *
 * Semantics: a recorded diff is `{path, oldText?, newText?}` — the call
 * replaced `oldText` with `newText` (absent `oldText` = the call created the
 * content, absent `newText` = it removed it). Devin records whole-file
 * before/after payloads; Cline `editor` inputs record search/replace hunks.
 * The revert rule is uniform: a step applies only when the recorded
 * after-state is still present verbatim, so a restore never clobbers drift —
 * it either lands exactly or skips with a reason.
 */

/** A diff folded into a restore, tagged with the call that produced it. */
export interface RecordedDiff {
  readonly toolCallId: string;
  readonly diff: ToolCallDiff;
}

/**
 * What a restore would do to one path — computed before any write so the
 * caller can report `skip`/`unchanged` without touching the filesystem.
 */
export type FileRestorePlan =
  | { readonly kind: "write"; readonly path: string; readonly content: string }
  | { readonly kind: "delete"; readonly path: string }
  | { readonly kind: "unchanged"; readonly path: string }
  | { readonly kind: "skip"; readonly path: string; readonly reason: string };

/**
 * Resolve a path a diff or request names against the session's working
 * directory. Returns `null` when the result escapes the directory — stores
 * record both absolute and relative spellings, and `../` is never trusted.
 */
export const resolveWorkspacePath = (cwd: string, path: string): string | null => {
  if (path.trim() === "") return null;
  const root = resolve(cwd);
  // `resolve` keeps an already-absolute path; a relative one lands under cwd.
  const resolved = isAbsolute(path) ? resolve(path) : resolve(root, path);
  return resolved === root || resolved.startsWith(root + sep) ? resolved : null;
};

const samePath = (cwd: string, a: string, b: string): boolean => {
  if (a === b) return true;
  const ra = resolveWorkspacePath(cwd, a);
  const rb = resolveWorkspacePath(cwd, b);
  return ra !== null && ra === rb;
};

/**
 * Every recorded diff for `path`, in session order (earliest first). Stores
 * spell paths inconsistently (absolute on Devin, tool-input verbatim on
 * Cline), so matching goes through cwd-normalized comparison with an exact
 * string fast path.
 */
export const diffsForPath = (session: Session, path: string): ReadonlyArray<RecordedDiff> => {
  const out: RecordedDiff[] = [];
  for (const node of session.nodes) {
    for (const call of node.toolCalls) {
      for (const diff of call.diffs) {
        if (samePath(session.workingDirectory, diff.path, path)) {
          out.push({ toolCallId: call.id, diff });
        }
      }
    }
  }
  return out;
};

const countOccurrences = (haystack: string, needle: string): number => {
  let count = 0;
  let from = 0;
  for (;;) {
    const at = haystack.indexOf(needle, from);
    if (at === -1) return count;
    count += 1;
    from = at + needle.length;
  }
};

type RevertStep = { readonly content: string | null } | { readonly reason: string };

/**
 * Reverse-apply one recorded diff to `content` (`null` = file absent).
 * Any ambiguity — the recorded after-state missing or matching several
 * positions — is a skip, never a guess: the file has drifted and writing
 * anything would destroy the drift.
 */
const revertStep = (content: string | null, diff: ToolCallDiff): RevertStep => {
  const { oldText, newText } = diff;
  if (newText === undefined) {
    // The call removed content — a file delete when `oldText` was the whole
    // file, a hunk removal otherwise. Either way the only safe revert is
    // resurrecting `oldText` when the file is now absent; mid-file position
    // is unrecoverable.
    if (oldText === undefined) return { reason: "the recorded diff carries no content" };
    if (content === null) return { content: oldText };
    return { reason: "cannot re-locate where the recorded removal happened" };
  }
  if (content === null) {
    // The file is absent now. `oldText` is the recorded before-state; when
    // the call created the file (no `oldText`) staying absent is correct.
    return { content: oldText ?? null };
  }
  // Whole-file fast path — also the only sane answer for an empty newText.
  if (content === newText) return { content: oldText ?? null };
  if (newText === "") {
    return { reason: "the recorded after-state is empty and the file has since changed" };
  }
  const occurrences = countOccurrences(content, newText);
  if (occurrences === 0) {
    return { reason: "file no longer contains the recorded after-state" };
  }
  if (occurrences > 1) {
    return { reason: "the recorded after-state matches multiple positions" };
  }
  return { content: content.replace(newText, oldText ?? "") };
};

/**
 * Plan a file restore from recorded diffs.
 *
 * With `toolCallId`, reverts exactly that call's change to `path`. Without
 * it, folds every recorded diff for the path in reverse — the result is the
 * file's state before the session first touched it — and any step that
 * can't reverse-apply cleanly skips the whole restore.
 */
export const planPathRestore = (
  session: Session,
  path: string,
  current: string | null,
  toolCallId?: string,
): FileRestorePlan => {
  const recorded = diffsForPath(session, path).filter(
    (entry) => toolCallId === undefined || entry.toolCallId === toolCallId,
  );
  if (recorded.length === 0) {
    return {
      kind: "skip",
      path,
      reason:
        toolCallId === undefined
          ? "no recorded change for this path"
          : `no recorded change for this path in call ${toolCallId}`,
    };
  }
  let content = current;
  for (const { diff } of [...recorded].reverse()) {
    const step = revertStep(content, diff);
    if ("reason" in step) return { kind: "skip", path, reason: step.reason };
    content = step.content;
  }
  if (content === null) {
    return current === null ? { kind: "unchanged", path } : { kind: "delete", path };
  }
  if (content === current) return { kind: "unchanged", path };
  return { kind: "write", path, content };
};

/* ------------------------------------------------------------------ */
/* file-history checkpoints (Claude)                                   */
/* ------------------------------------------------------------------ */

/**
 * `CheckpointRef.kind` a Claude `file-history-snapshot` entry maps to — the
 * ref is the snapshot's `messageId`, and the path→backup map it covers rides
 * on `session.metadata.fileHistory` (read by `fileHistorySnapshot`). Unlike
 * a shadow-git ref the payload lives outside the repo, in the agent's own
 * `file-history/<sessionId>/` dir.
 */
export const FILE_HISTORY_KIND = "file-history-snapshot";

/**
 * One tracked file's backup pointer: the blob name under
 * `file-history/<sessionId>/`, or `null` for the deletion tombstone — the
 * file did not exist at that checkpoint.
 */
export interface FileHistoryBackup {
  readonly backup: string | null;
  readonly version?: number;
}

/**
 * Tolerant read of `session.metadata.fileHistory` for one checkpoint ref.
 * `sessionId` names the owning session's backup dir (a subagent's entries
 * name the parent session); `files` is the workspace-absolute path → backup
 * map the checkpoint covers. `undefined` when the store recorded no map —
 * the ref then has nothing a restore can materialize from.
 */
export const fileHistorySnapshot = (
  session: Session,
  ref: string,
):
  | { readonly sessionId: string; readonly files: Record<string, FileHistoryBackup> }
  | undefined => {
  const meta = session.metadata;
  if (meta === null || typeof meta !== "object") return undefined;
  const history = (meta as Record<string, unknown>).fileHistory;
  if (history === null || typeof history !== "object") return undefined;
  const sessionId = (history as Record<string, unknown>).sessionId;
  const snapshots = (history as Record<string, unknown>).snapshots;
  if (typeof sessionId !== "string" || snapshots === null || typeof snapshots !== "object") {
    return undefined;
  }
  const entry = (snapshots as Record<string, unknown>)[ref];
  if (entry === null || typeof entry !== "object") return undefined;
  const rawFiles = (entry as Record<string, unknown>).files;
  if (rawFiles === null || typeof rawFiles !== "object") return undefined;
  const files: Record<string, FileHistoryBackup> = {};
  for (const [path, value] of Object.entries(rawFiles)) {
    if (value === null || typeof value !== "object") continue;
    const backup = (value as Record<string, unknown>).backup;
    if (backup !== null && typeof backup !== "string") continue;
    const version = (value as Record<string, unknown>).version;
    files[path] = {
      backup,
      ...(typeof version === "number" && Number.isFinite(version) ? { version } : {}),
    };
  }
  return { sessionId, files };
};
