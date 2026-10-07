import { accessSync, constants, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import {
  SERVICE_NAME,
  envTemplate,
  type ServiceBackend,
  type ServiceSpec,
  type ServiceStatus,
} from "./backend.js";

/**
 * systemd backend — per-user units at `~/.config/systemd/user` (the default:
 * no root, `EnvironmentFile` can use the `%h` specifier) or system units at
 * `/etc/systemd/system`. `enable --now` starts the service on install and at
 * boot/login; `disable --now` stops it. `systemctl`/`journalctl` are resolved
 * via PATH so tests can drive the backend against stub binaries.
 */

const UNIT = `${SERVICE_NAME}.service`;

/** systemd escapes ExecStart args by double-quoting (never single-quoting). */
const quoteArg = (arg: string): string =>
  /[\s"']/.test(arg) ? `"${arg.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"` : arg;

const onPath = (bin: string): boolean =>
  (process.env.PATH ?? "").split(delimiter).some((dir) => {
    if (dir === "") return false;
    try {
      accessSync(join(dir, bin), constants.X_OK);
      return true;
    } catch {
      return false;
    }
  });

const requireSystemd = (bin = "systemctl"): void => {
  if (!onPath(bin)) {
    throw new Error(`systemd not available — \`${bin}\` is not on PATH`);
  }
};

/** `systemctl` argv — `--user` for per-user units, plain for system units. */
const systemctl = (spec: ServiceSpec, args: ReadonlyArray<string>): string[] => [
  "systemctl",
  ...(spec.system ? [] : ["--user"]),
  ...args,
];

const journalctl = (spec: ServiceSpec, args: ReadonlyArray<string>): string[] => [
  "journalctl",
  ...(spec.system ? [] : ["--user"]),
  ...args,
];

const logCmd = (cmd: ReadonlyArray<string>): void => {
  console.error(`systemd: $ ${cmd.join(" ")}`);
};

// `env: process.env` is explicit — Bun defaults to the launch-time env, but
// tests (and embedders) mutate PATH/HOME after boot.
const run = (cmd: ReadonlyArray<string>): void => {
  logCmd(cmd);
  const proc = Bun.spawnSync([...cmd], {
    env: process.env,
    stdin: "ignore",
    stdout: "inherit",
    stderr: "pipe",
  });
  if (proc.exitCode !== 0) {
    const detail = proc.stderr.toString().trim();
    throw new Error(`\`${cmd.join(" ")}\` exited ${proc.exitCode}${detail ? `: ${detail}` : ""}`);
  }
};

/** Same as run() but tolerant — returns the exit code instead of throwing. */
const tryRun = (cmd: ReadonlyArray<string>): number => {
  logCmd(cmd);
  return Bun.spawnSync([...cmd], {
    env: process.env,
    stdin: "ignore",
    stdout: "inherit",
    stderr: "pipe",
  }).exitCode;
};

const capture = (cmd: ReadonlyArray<string>): { code: number; stdout: string; stderr: string } => {
  const proc = Bun.spawnSync([...cmd], {
    env: process.env,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  return {
    code: proc.exitCode,
    stdout: proc.stdout.toString(),
    stderr: proc.stderr.toString(),
  };
};

const NOT_INSTALLED: ServiceStatus = {
  installed: false,
  enabled: false,
  active: false,
  detail: "",
};

// `is-enabled` prints one of these to stdout when the unit is known; a missing
// unit exits non-zero with nothing on stdout.
const KNOWN_UNIT_STATE =
  /^(enabled|linked|static|indirect|generated|transient|alias|masked|disabled)/;

export const systemdBackend: ServiceBackend = {
  id: "systemd",

  unitPath: (spec) =>
    spec.system
      ? `/etc/systemd/system/${UNIT}`
      : join(homedir(), ".config", "systemd", "user", UNIT),

  render: (spec) => {
    // User units may use the %h specifier (the service manager resolves it);
    // system units can't — a literal absolute path is required there.
    const home = homedir();
    const envFile =
      !spec.system && spec.envFile.startsWith(`${home}/`)
        ? `%h${spec.envFile.slice(home.length)}`
        : spec.envFile;
    return [
      "[Unit]",
      "Description=Sepia node",
      "After=network-online.target",
      "",
      "[Service]",
      `ExecStart=${spec.exec.map(quoteArg).join(" ")}`,
      `EnvironmentFile=-${envFile}`,
      "Restart=on-failure",
      "RestartSec=2",
      "",
      "[Install]",
      `WantedBy=${spec.system ? "multi-user.target" : "default.target"}`,
      "",
    ].join("\n");
  },

  install: async (spec) => {
    requireSystemd();
    const unit = systemdBackend.unitPath(spec);
    mkdirSync(dirname(unit), { recursive: true });
    writeFileSync(unit, systemdBackend.render(spec));
    // the env template is written once — never clobber an admin's edits
    if (!existsSync(spec.envFile)) {
      mkdirSync(dirname(spec.envFile), { recursive: true });
      writeFileSync(spec.envFile, envTemplate());
    }
    run(systemctl(spec, ["daemon-reload"]));
    // A prior crash loop trips the start limit — clear it or the enable
    // refuses with 'Unit ... failed' even though the unit is healthy now.
    tryRun(systemctl(spec, ["reset-failed", SERVICE_NAME]));
    run(systemctl(spec, ["enable", "--now", SERVICE_NAME]));
  },

  uninstall: async (spec, purge) => {
    requireSystemd();
    // tolerate a missing/partial unit — removing the file is the source of truth
    tryRun(systemctl(spec, ["disable", "--now", SERVICE_NAME]));
    rmSync(systemdBackend.unitPath(spec), { force: true });
    run(systemctl(spec, ["daemon-reload"]));
    if (purge) rmSync(spec.envFile, { force: true });
  },

  status: async (spec) => {
    if (!onPath("systemctl")) return NOT_INSTALLED;

    const enabledProc = capture(systemctl(spec, ["is-enabled", SERVICE_NAME]));
    const enabledState = enabledProc.stdout.trim();
    const installed =
      existsSync(systemdBackend.unitPath(spec)) || KNOWN_UNIT_STATE.test(enabledState);
    if (!installed) return NOT_INSTALLED;

    const activeProc = capture(systemctl(spec, ["is-active", SERVICE_NAME]));
    const pidProc = capture(systemctl(spec, ["show", SERVICE_NAME, "-p", "MainPID"]));
    const pid = Number(/^MainPID=(\d+)/m.exec(pidProc.stdout)?.[1] ?? "0");
    const detail = onPath("journalctl")
      ? capture(journalctl(spec, ["-u", SERVICE_NAME, "-n", "10", "--no-pager"])).stdout.trim()
      : "";
    return {
      installed: true,
      enabled: enabledState.startsWith("enabled"),
      active: activeProc.stdout.trim() === "active",
      ...(pid > 0 ? { pid } : {}),
      detail,
    };
  },

  restart: async (spec) => {
    requireSystemd();
    run(systemctl(spec, ["restart", SERVICE_NAME]));
  },

  logs: async (spec, follow) => {
    requireSystemd("journalctl");
    const cmd = journalctl(spec, ["-u", SERVICE_NAME, ...(follow ? ["-f"] : [])]);
    logCmd(cmd);
    return Bun.spawnSync(cmd, {
      env: process.env,
      stdin: "inherit",
      stdout: "inherit",
      stderr: "inherit",
    }).exitCode;
  },
};
