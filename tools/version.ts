#!/usr/bin/env bun
/**
 * Stamps the release version — `MAJOR.YYMMDD.HHMM` computed from the current
 * UTC time (e.g. `0.261005.1330` for 2026-10-05 13:30; `0.x` = unstable).
 *
 * Note: HHMM is written as a plain integer — semver forbids leading zeros, so
 * 00:45 stamps as `0.261005.45`, not `0.261005.0045`. npm rejects the padded
 * form outright; keep versions strictly `MAJOR.YYMMDD.<int HHMM>`.
 *
 * The stamp lands everywhere the version needs to live:
 *
 *   - VERSION                    — repo-level stamp file
 *   - <workspace>/package.json   — every workspace `version` field, rewritten
 *     surgically so formatting survives. `apps/server`'s feeds `GET /api/node`
 *     (src/node.ts imports package.json) and `sepia --version`; the
 *     publishable packages' (`packages/sepia-node`, `packages/sepia-ui`) is
 *     what `npm publish` ships.
 *
 * Afterwards it runs `bun install` — bun.lock records workspace versions, so
 * a stamp that skips this leaves `bun install --frozen-lockfile` broken.
 *
 * Usage:
 *   bun tools/version.ts            # stamp now + sync bun.lock
 *   bun tools/version.ts --print    # print the current stamp, write nothing
 *   bun tools/version.ts --no-install
 */
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const versionFile = join(repoRoot, "VERSION");

const args = process.argv.slice(2);
const printOnly = args.includes("--print");
const skipInstall = args.includes("--no-install");

/** `0.YYMMDD.HHMM` — HHMM as an integer (semver has no leading zeros). */
const stamp = (now: Date): string => {
  const pad = (n: number): string => String(n).padStart(2, "0");
  const yymmdd = `${pad(now.getUTCFullYear() % 100)}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}`;
  const hhmm = Number(`${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}`);
  return `0.${yymmdd}.${hhmm}`;
};

/** Every workspace package.json — root plus the workspaces globs. */
const packageJsonPaths = async (): Promise<string[]> => {
  const paths = [join(repoRoot, "package.json")];
  for (const group of ["apps", "packages", "tools"]) {
    const dir = join(repoRoot, group);
    let entries: string[] = [];
    try {
      entries = await readdir(dir);
    } catch {
      continue;
    }
    for (const entry of entries.sort()) {
      paths.push(join(dir, entry, "package.json"));
    }
  }
  return paths;
};

/** Rewrites only the `"version": "..."` line so file formatting survives. */
const stampPackageJson = async (path: string, version: string): Promise<boolean> => {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch {
    return false;
  }
  const next = text.replace(/("version"\s*:\s*")[^"]*(")/, `$1${version}$2`);
  if (next === text) return false;
  await writeFile(path, next);
  return true;
};

if (printOnly) {
  try {
    console.log((await readFile(versionFile, "utf8")).trim());
  } catch {
    console.log("0.0.0");
  }
  process.exit(0);
}

const version = stamp(new Date());

let stamped = 0;
for (const path of await packageJsonPaths()) {
  if (await stampPackageJson(path, version)) {
    stamped += 1;
    console.log(`  ${path.slice(repoRoot.length + 1)}`);
  }
}
await writeFile(versionFile, `${version}\n`);

console.log(`version: stamped ${version} into VERSION + ${stamped} package.json file(s)`);

if (!skipInstall) {
  // bun.lock carries the workspace version table — refresh it or
  // `--frozen-lockfile` installs fail on the next run.
  const proc = Bun.spawnSync(["bun", "install"], {
    cwd: repoRoot,
    stdio: ["inherit", "inherit", "inherit"],
  });
  if (proc.exitCode !== 0) {
    console.error(`version: bun install failed (${proc.exitCode}) — bun.lock may be stale`);
    process.exit(proc.exitCode ?? 1);
  }
}
