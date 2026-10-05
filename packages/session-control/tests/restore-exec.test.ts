/**
 * `defaultRestoreExec` — the real fs/git seam `ControlPlane.restore` uses.
 * Runs against temp dirs and a real `git` binary; no mocks.
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import { defaultRestoreExec } from "../src/restore-exec.js";

const dir = mkdtempSync(join(tmpdir(), "sepia-restore-exec-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

test("readFile returns bytes for a file and null for a missing one", async () => {
  const file = join(dir, "a.txt");
  await defaultRestoreExec.writeFile(file, new Uint8Array([104, 105]));
  const bytes = await defaultRestoreExec.readFile(file);
  expect(bytes).not.toBeNull();
  expect(new TextDecoder().decode(bytes ?? undefined)).toBe("hi");
  expect(await defaultRestoreExec.readFile(join(dir, "nope.txt"))).toBeNull();
  // a directory is unreadable as a file → absent
  expect(await defaultRestoreExec.readFile(dir)).toBeNull();
});

test("writeFile creates parent directories; removeFile deletes and tolerates absence", async () => {
  const nested = join(dir, "deep", "down", "b.txt");
  await defaultRestoreExec.writeFile(nested, new Uint8Array([65]));
  expect(await defaultRestoreExec.readFile(nested)).not.toBeNull();
  await defaultRestoreExec.removeFile(nested);
  expect(await defaultRestoreExec.readFile(nested)).toBeNull();
  // force:true — removing a missing file is a no-op
  await defaultRestoreExec.removeFile(nested);
});

test("git reports a zero exit for a repo and a non-zero exit without failing", async () => {
  const repo = join(dir, "repo");
  const init = await defaultRestoreExec.git(dir, ["init", repo]);
  expect(init.code).toBe(0);
  const status = await defaultRestoreExec.git(repo, ["status", "--porcelain"]);
  expect(status.code).toBe(0);
  // not a repository → git answers with a non-zero exit, not an error
  const outside = await defaultRestoreExec.git(dir, ["status"]);
  expect(outside.code).not.toBe(0);
  expect(outside.stderr).toContain("not a git repository");
});
