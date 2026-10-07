import { spawnSync as nodeSpawnSync, spawn as nodeSpawn } from "node:child_process";
import {
  accessSync,
  constants,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { envTemplate, type ServiceBackend, type ServiceSpec } from "./backend.js";

/**
 * launchd backend: one `ai.sepia` job — a LaunchAgent under
 * `~/Library/LaunchAgents` for `--user` (the only sensible default for a dev
 * tool) or a LaunchDaemon under `/Library/LaunchDaemons` for `--system`.
 * Unlike systemd, launchd has no EnvironmentFile directive, so the env file
 * is rendered into the plist's `EnvironmentVariables` dict at install time —
 * edit the file, re-run `sepia service install` to apply.
 */

const LABEL = "ai.sepia";
/** `status` shows the last N log lines; `logs` prints the last N. */
const STATUS_LOG_LINES = 10;
const LOG_LINES = 50;

const uid = (): number => process.getuid?.() ?? 0;

/** The launchd domain a spec loads into: `gui/<uid>` or `system`. */
const domain = (spec: ServiceSpec): string => (spec.system ? "system" : `gui/${uid()}`);

/** The service endpoint `print`/`kickstart` take: `gui/<uid>/ai.sepia`. */
const serviceTarget = (spec: ServiceSpec): string => `${domain(spec)}/${LABEL}`;

const logPath = (spec: ServiceSpec): string =>
  spec.system ? "/var/log/sepia.log" : `${homedir()}/Library/Logs/sepia.log`;

const unitPath = (spec: ServiceSpec): string =>
  spec.system
    ? `/Library/LaunchDaemons/${LABEL}.plist`
    : `${homedir()}/Library/LaunchAgents/${LABEL}.plist`;

// MARK: env file → plist

const xmlEscape = (text: string): string =>
  text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

const ENV_KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * Parse `KEY=value` lines out of the env file — blanks and `#` comments are
 * ignored, a missing file yields no vars (install still renders a valid
 * plist). Duplicate keys collapse to the last value.
 */
const parseEnvFile = (path: string): ReadonlyArray<readonly [string, string]> => {
  let raw: string;
  try {
    raw = readFileSync(path, "utf-8");
  } catch {
    return [];
  }
  const entries = new Map<string, string>();
  for (const line of raw.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) {
      continue;
    }
    const eq = trimmed.indexOf("=");
    if (eq <= 0) {
      continue;
    }
    const key = trimmed.slice(0, eq).trim();
    if (!ENV_KEY.test(key)) {
      continue;
    }
    entries.set(key, trimmed.slice(eq + 1).trim());
  }
  return [...entries];
};

const render = (spec: ServiceSpec): string => {
  const env = parseEnvFile(spec.envFile);
  const args = spec.exec.map((arg) => `\t\t<string>${xmlEscape(arg)}</string>`).join("\n");
  const envBlock =
    env.length === 0
      ? ""
      : `\t<key>EnvironmentVariables</key>\n\t<dict>\n${env
          .map(
            ([key, value]) =>
              `\t\t<key>${xmlEscape(key)}</key>\n\t\t<string>${xmlEscape(value)}</string>`,
          )
          .join("\n")}\n\t</dict>\n`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
\t<key>Label</key>
\t<string>${LABEL}</string>
\t<key>ProgramArguments</key>
\t<array>
${args}
\t</array>
${envBlock}\t<key>RunAtLoad</key>
\t<true/>
\t<key>KeepAlive</key>
\t<dict>
\t\t<key>SuccessfulExit</key>
\t\t<false/>
\t</dict>
\t<key>StandardOutPath</key>
\t<string>${xmlEscape(logPath(spec))}</string>
\t<key>StandardErrorPath</key>
\t<string>${xmlEscape(logPath(spec))}</string>
</dict>
</plist>
`;
};

// MARK: launchctl

/** `bin` resolves on PATH — the install preflight's whole question. */
const onPath = (bin: string): boolean =>
  (process.env.PATH ?? "")
    .split(delimiter)
    .filter((dir) => dir !== "")
    .some((dir) => {
      try {
        accessSync(join(dir, bin), constants.X_OK);
        return true;
      } catch {
        return false;
      }
    });

const requireLaunchctl = (): void => {
  if (!onPath("launchctl")) {
    throw new Error("launchd: `launchctl` not found on PATH — is this macOS?");
  }
};

interface Spawned {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
}

/**
 * One launchctl invocation, echoed before it runs. `Bun.spawnSync` in the
 * shipped binary; the node fallback keeps the backend exercisable under
 * vitest (where `Bun` is undefined) — the launchctl stubs the tests put on
 * PATH see identical argv either way.
 */
const spawn = (argv: ReadonlyArray<string>): Spawned => {
  console.log(`launchd: $ ${argv.join(" ")}`);
  if (typeof Bun !== "undefined") {
    const out = Bun.spawnSync([...argv], { stdout: "pipe", stderr: "pipe" });
    return {
      code: out.exitCode,
      stdout: out.stdout.toString(),
      stderr: out.stderr.toString(),
    };
  }
  const out = nodeSpawnSync(argv[0] ?? "", argv.slice(1), { encoding: "utf-8" });
  return {
    code: out.status ?? 1,
    stdout: out.stdout ?? "",
    stderr: out.stderr ?? (out.error === undefined ? "" : String(out.error)),
  };
};

/** Throwing variant — the command must succeed or the verb fails. */
const run = (argv: ReadonlyArray<string>): Spawned => {
  const out = spawn(argv);
  if (out.code !== 0) {
    throw new Error(
      `launchd: \`${argv.join(" ")}\` exited ${out.code}: ${out.stderr.trim() || out.stdout.trim()}`,
    );
  }
  return out;
};

/** Whether launchd currently has the job loaded (`print` probes it). */
const loaded = (spec: ServiceSpec): boolean =>
  spawn(["launchctl", "print", serviceTarget(spec)]).code === 0;

/** Last `count` lines of a log file — "" when the file isn't there. */
const tailLines = (path: string, count: number): string => {
  try {
    const lines = readFileSync(path, "utf-8").split("\n");
    while (lines.length > 0 && lines[lines.length - 1] === "") {
      lines.pop();
    }
    return lines.slice(-count).join("\n");
  } catch {
    return "";
  }
};

export const launchdBackend: ServiceBackend = {
  id: "launchd",
  unitPath,
  render,

  /**
   * Write the env template (first install only — the file is user state, a
   * re-install only re-renders it into the plist), write the plist, then
   * bootstrap into the gui/system domain. Re-installing over a loaded job
   * bootouts first so the new plist actually takes.
   */
  install: async (spec) => {
    requireLaunchctl();
    const plist = unitPath(spec);
    mkdirSync(dirname(plist), { recursive: true });
    mkdirSync(dirname(logPath(spec)), { recursive: true });
    mkdirSync(dirname(spec.envFile), { recursive: true });
    if (!existsSync(spec.envFile)) {
      writeFileSync(spec.envFile, envTemplate());
    }
    writeFileSync(plist, render(spec));
    if (loaded(spec)) {
      spawn(["launchctl", "bootout", domain(spec), plist]);
    }
    run(["launchctl", "bootstrap", domain(spec), plist]);
  },

  /** bootout is best-effort — "not loaded" isn't a failure to remove files. */
  uninstall: async (spec, purge) => {
    if (onPath("launchctl")) {
      spawn(["launchctl", "bootout", domain(spec), unitPath(spec)]);
    }
    rmSync(unitPath(spec), { force: true });
    if (purge) {
      rmSync(spec.envFile, { force: true });
    }
  },

  /**
   * `launchctl print <domain>/<label>` is the modern state query — a nonzero
   * exit means the job isn't loaded. `installed` tracks launchd's view;
   * `enabled` tracks the plist file (presence = it loads at login/boot).
   */
  status: async (spec) => {
    const enabled = existsSync(unitPath(spec));
    const detail = tailLines(logPath(spec), STATUS_LOG_LINES);
    if (!onPath("launchctl")) {
      return { installed: false, enabled, active: false, detail };
    }
    const out = spawn(["launchctl", "print", serviceTarget(spec)]);
    if (out.code !== 0) {
      return { installed: false, enabled, active: false, detail };
    }
    const state = /^\s*state = (.+)$/m.exec(out.stdout)?.[1]?.trim();
    const pid = /^\s*pid = (\d+)$/m.exec(out.stdout)?.[1];
    const pidNumber = pid === undefined ? undefined : Number(pid);
    return {
      installed: true,
      enabled,
      active: state === "running" || (pidNumber !== undefined && pidNumber > 0),
      pid: pidNumber === 0 ? undefined : pidNumber,
      detail,
    };
  },

  /** kickstart -k = kill the running job and start it again on the new plist. */
  restart: async (spec) => {
    requireLaunchctl();
    run(["launchctl", "kickstart", "-k", serviceTarget(spec)]);
  },

  /** Print the tail; `--follow` hands the terminal to `tail -f`. */
  logs: async (spec, follow) => {
    const log = logPath(spec);
    if (follow) {
      return await new Promise<number>((resolve, reject) => {
        const child = nodeSpawn("tail", ["-n", `${LOG_LINES}`, "-f", log], {
          stdio: "inherit",
        });
        child.on("error", reject);
        child.on("exit", (code) => resolve(code ?? 0));
      });
    }
    const tail = tailLines(log, LOG_LINES);
    if (tail !== "") {
      console.log(tail);
    }
    return 0;
  },
};
