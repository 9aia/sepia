import { execFile } from "node:child_process";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import type { RestoreExec } from "./types.js";

// Checkpoint blobs can be whole workspaces worth of file content — a bounded
// buffer keeps a pathological ref from growing the process without limit.
const GIT_MAX_BUFFER = 64 * 1024 * 1024;

const runGit = (
  cwd: string,
  args: ReadonlyArray<string>,
): Promise<{ code: number; stdout: Uint8Array; stderr: string }> =>
  new Promise((resolvePromise, reject) => {
    execFile(
      "git",
      ["-C", cwd, ...args],
      { encoding: "buffer", maxBuffer: GIT_MAX_BUFFER },
      (error, stdout, stderr) => {
        if (error !== null && typeof error.code !== "number") {
          // Spawn failures (ENOENT, EACCES) are real errors; a non-zero exit
          // is just git answering "no" — reported via `code` instead.
          reject(error);
          return;
        }
        resolvePromise({
          code: typeof error?.code === "number" ? error.code : 0,
          stdout: new Uint8Array(stdout),
          stderr: stderr.toString(),
        });
      },
    );
  });

/**
 * The real disk/git seam for `ControlPlane.restore` — plain node fs plus the
 * workspace's own `git` binary (checkpoint refs live in its object store).
 */
export const defaultRestoreExec: RestoreExec = {
  readFile: async (path) => {
    try {
      return new Uint8Array(await readFile(path));
    } catch {
      // Any failure reads as "absent": ENOENT is the common case; an
      // unreadable file simply can't have its after-state verified anyway.
      return null;
    }
  },
  writeFile: async (path, content) => {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, content);
  },
  removeFile: (path) => rm(path, { force: true }),
  git: runGit,
};
