#!/usr/bin/env bun
/**
 * One-shot release — stamps the date version, builds both distribution forms,
 * then publishes with the standard tools (`npm publish`, `git tag`, `gh
 * release`). No release framework — the stamp model (`MAJOR.YYMMDD.HHMM`)
 * doesn't map onto semver-increment tooling.
 *
 * Steps, in order (each stops the run on failure):
 *   1. `tools/version.ts`               — stamp into VERSION + every
 *                                       package.json + bun.lock
 *   2. `tools/build-npm.ts`             — stage packages/sepia-{node,ui}
 *   3. `tools/build-binary.ts`          — `bun build --compile` → ./sepia
 *                                       (host platform; skipped w/ --no-binary)
 *   4. commit "release v<stamp>" + `git tag v<stamp>` + push --follow-tags
 *   5. `bun publish --access public`    — packages/sepia-node, sepia-ui
 *                                       (`bun`, not `npm`: the repo's
 *                                       devEngines declares bun; npm 11
 *                                       refuses on the mismatch)
 *   6. `gh release create v<stamp>`     — notes auto-generated from commits;
 *                                       the binary uploads as an asset
 *
 * Flags:
 *   --dry-run        — full pipeline except the irreversibles: no commit/tag/
 *                      push, `bun publish --dry-run`, no `gh release create`
 *   --version X.Y.Z  — override the date stamp
 *   --no-binary      — skip the compiled binary (still publishes npm)
 *   --no-npm         — skip npm publish
 *   --no-gh          — skip the GitHub release
 *
 * Preflight fails fast on: dirty tree, non-master branch, missing `npm whoami`
 * / `gh auth status` (auth checks are skipped in --dry-run). Credentials are
 * the standard npm registry ones — `npm whoami` covers `bun publish` since
 * bun reads ~/.npmrc.
 */
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const args = process.argv.slice(2);

const dryRun = args.includes("--dry-run");
const noBinary = args.includes("--no-binary");
const noNpm = args.includes("--no-npm");
const noGh = args.includes("--no-gh");
const versionIndex = args.indexOf("--version");
const versionOverride = versionIndex !== -1 ? args[versionIndex + 1] : undefined;
if (versionIndex !== -1 && !versionOverride) {
  console.error("release: --version expects a value");
  process.exit(1);
}

const out = (argv: string[]): string =>
  Bun.spawnSync(argv, { cwd: repoRoot, stdout: "pipe", stderr: "pipe" }).stdout.toString().trim();

const check = (argv: string[], hint: string): void => {
  const proc = Bun.spawnSync(argv, { cwd: repoRoot, stdio: ["ignore", "pipe", "pipe"] });
  if (proc.exitCode !== 0) {
    console.error(`release: preflight failed — ${hint}`);
    console.error(`  $ ${argv.join(" ")}\n  ${proc.stderr.toString().trim()}`);
    process.exit(1);
  }
};

const run = (argv: string[], cwd = repoRoot): void => {
  console.log(`release: $ ${argv.join(" ")}  (in ${cwd})`);
  const proc = Bun.spawnSync(argv, { cwd, stdio: ["inherit", "inherit", "inherit"] });
  if (proc.exitCode !== 0) {
    console.error(`release: failed (${proc.exitCode}): ${argv.join(" ")}`);
    process.exit(proc.exitCode ?? 1);
  }
};

/* ---- preflight -------------------------------------------------------- */

if (!dryRun) {
  const dirty = out(["git", "status", "--porcelain"]);
  if (dirty !== "") {
    console.error(`release: working tree is dirty — commit or stash first:\n${dirty}`);
    process.exit(1);
  }
  const branch = out(["git", "rev-parse", "--abbrev-ref", "HEAD"]);
  if (branch !== "master") {
    console.error(`release: releases cut from master, currently on "${branch}"`);
    process.exit(1);
  }
  check(
    ["npm", "whoami"],
    "not logged into the npm registry (npm login — bun publish reads ~/.npmrc)",
  );
  if (!noGh) check(["gh", "auth", "status"], "gh CLI not authenticated (gh auth login)");
}

/* ---- version stamp ---------------------------------------------------- */

let version: string;
if (versionOverride !== undefined) {
  version = versionOverride;
  console.log(`release: version override ${version}`);
  if (!dryRun) run(["bun", "tools/version.ts", version]);
} else if (dryRun) {
  version = out(["bun", "tools/version.ts", "--print-next"]);
} else {
  run(["bun", "tools/version.ts"]);
  version = out(["bun", "tools/version.ts", "--print"]);
}
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`release: bad version "${version}" — expected MAJOR.MINOR.PATCH`);
  process.exit(1);
}
const tag = `v${version}`;
console.log(`release: version ${version} (${tag})`);

/* ---- build ------------------------------------------------------------ */

run(["bun", "tools/build-npm.ts"]);
if (!noBinary) run(["bun", "tools/build-binary.ts", "--skip-web-build"]);

if (dryRun) {
  console.log("release: [dry-run] would commit, tag, push, npm publish, gh release create");
  for (const pkg of ["sepia-node", "sepia-ui"]) {
    // `bun publish <arg>` expects a tarball, not a dir — run inside the package.
    run(["bun", "publish", "--access", "public", "--dry-run"], join(repoRoot, "packages", pkg));
  }
  console.log(`release: [dry-run] done — ${tag} is staged but nothing was pushed`);
  process.exit(0);
}

/* ---- commit, tag, push ------------------------------------------------ */

run(["git", "add", "-A"]);
run(["git", "commit", "-m", `release ${tag}`]);
run(["git", "tag", tag]);
run(["git", "push", "--follow-tags"]);

/* ---- publish ---------------------------------------------------------- */

if (!noNpm) {
  for (const pkg of ["sepia-node", "sepia-ui"]) {
    run(["bun", "publish", "--access", "public"], join(repoRoot, "packages", pkg));
  }
}

if (!noGh) {
  const ghArgs = ["gh", "release", "create", tag, "--title", tag, "--generate-notes"];
  if (!noBinary) ghArgs.push("./sepia");
  run(ghArgs);
}

console.log(`release: ${tag} shipped`);
