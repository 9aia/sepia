#!/usr/bin/env bun
/**
 * Stages the two publishable npm packages (packages/sepia-node,
 * packages/sepia-ui) — source + dist, platform-neutral, no `bun --compile`.
 *
 *   1. `vp run sepia-web#build` → apps/web/dist/client (skip with
 *      `--skip-web-build`; the release workflow reuses the `ready` build)
 *   2. sepia-node: `bun build apps/sepia/src/main.ts --target bun --minify`
 *      → `dist/cli.js` — one ~3MB file bundling the CLI, server and all
 *      workspace deps (`workspace:*` never reaches the published manifest;
 *      `bun:*` builtins stay external — Bun is the runtime). The web bundle
 *      copies to `ui/`; `bin/sepia.js` defaults SEPIA_UI_DIR to it, so the
 *      npm artifact serves the UI from disk rather than the compiled
 *      binary's $bunfs embed.
 *   3. sepia-ui: the same web bundle at `dist/` for self-hosters.
 *
 * Stamp versions first (`vp run version:bump`) — the bundles inline
 * apps/server/package.json's `version` at build time.
 *
 * Usage: bun tools/build-npm.ts [--skip-web-build] [--only sepia-node|sepia-ui]
 */
import { cp, mkdir, readFile, rm } from "node:fs/promises";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const distDir = join(repoRoot, "apps/web/dist/client");
const cliEntry = join(repoRoot, "apps/sepia/src/main.ts");

const PACKAGES = ["sepia-node", "sepia-ui"] as const;
type PackageName = (typeof PACKAGES)[number];

const args = process.argv.slice(2);
const skipWebBuild = args.includes("--skip-web-build");
const onlyIndex = args.indexOf("--only");
const only: PackageName | undefined =
  onlyIndex !== -1 ? (args[onlyIndex + 1] as PackageName) : undefined;
if (only !== undefined && !PACKAGES.includes(only)) {
  console.error(`build-npm: --only expects one of ${PACKAGES.join(", ")}, got "${only}"`);
  process.exit(1);
}
const wanted = (name: PackageName): boolean => only === undefined || only === name;

const run = (argv: string[]): void => {
  const proc = Bun.spawnSync(argv, { cwd: repoRoot, stdio: ["inherit", "inherit", "inherit"] });
  if (proc.exitCode !== 0) {
    console.error(`build-npm: command failed (${proc.exitCode}): ${argv.join(" ")}`);
    process.exit(proc.exitCode ?? 1);
  }
};

const packageDir = (name: PackageName): string => join(repoRoot, "packages", name);

const versionOf = async (name: PackageName): Promise<string> => {
  const pkg = JSON.parse(await readFile(join(packageDir(name), "package.json"), "utf8")) as {
    version?: string;
  };
  return pkg.version ?? "0.0.0";
};

if (!skipWebBuild) {
  console.log("build-npm: building web bundle (vp run sepia-web#build)");
  run(["vp", "run", "sepia-web#build"]);
}

if (!(await Bun.file(join(distDir, "index.html")).exists())) {
  console.error(
    `build-npm: ${distDir} has no index.html — run the web build first (or drop --skip-web-build)`,
  );
  process.exit(1);
}

if (wanted("sepia-node")) {
  const dir = packageDir("sepia-node");
  console.log("build-npm: bundling apps/sepia/src/main.ts -> packages/sepia-node/dist/cli.js");
  await mkdir(join(dir, "dist"), { recursive: true });
  run([
    "bun",
    "build",
    cliEntry,
    "--target",
    "bun",
    "--minify",
    "--outfile",
    join(dir, "dist/cli.js"),
  ]);

  await rm(join(dir, "ui"), { recursive: true, force: true });
  await cp(distDir, join(dir, "ui"), { recursive: true });
}

if (wanted("sepia-ui")) {
  const dir = packageDir("sepia-ui");
  await rm(join(dir, "dist"), { recursive: true, force: true });
  await cp(distDir, join(dir, "dist"), { recursive: true });
}

for (const name of PACKAGES.filter(wanted)) {
  const version = await versionOf(name);
  console.log(`build-npm: staged ${name}@${version}`);
  if (version === "0.0.0") {
    console.warn(
      `build-npm: warning — ${name} is still at the 0.0.0 placeholder; run \`vp run version:bump\` before publishing`,
    );
  }
}
