/**
 * Service-manager backends for `sepia service *`. Each backend owns the
 * platform's unit file layout + control tool (`systemctl`, `launchctl`)
 * behind one interface — the CLI surface never branches on platform.
 */

export interface ServiceSpec {
  /** Absolute path the unit's ExecStart/ProgramArguments should run. */
  readonly exec: ReadonlyArray<string>;
  /** Optional env file — systemd reads it directly; launchd renders it. */
  readonly envFile: string;
  /** true → system-level (root) unit; false → per-user unit. */
  readonly system: boolean;
}

export interface ServiceStatus {
  readonly installed: boolean;
  readonly enabled: boolean;
  readonly active: boolean;
  readonly pid?: number;
  readonly detail: string;
}

export interface ServiceBackend {
  readonly id: "systemd" | "launchd";
  /** Where the unit/plist file lives for this spec. */
  readonly unitPath: (spec: ServiceSpec) => string;
  /** Render the unit/plist content — pure, snapshot-tested. */
  readonly render: (spec: ServiceSpec) => string;
  /** Write unit + env template, reload, enable + start. Idempotent. */
  readonly install: (spec: ServiceSpec) => Promise<void>;
  /** Stop, disable, remove the unit. `purge` drops the env file too. */
  readonly uninstall: (spec: ServiceSpec, purge: boolean) => Promise<void>;
  readonly status: (spec: ServiceSpec) => Promise<ServiceStatus>;
  readonly restart: (spec: ServiceSpec) => Promise<void>;
  readonly logs: (spec: ServiceSpec, follow: boolean) => Promise<number>;
}

/** The unit name both backends use: sepia.service / ai.sepia.plist. */
export const SERVICE_NAME = "sepia";

/** Where `~/.config/sepia/env` lives — the EnvironmentFile/template. */
export const defaultEnvFile = (home: string): string => `${home}/.config/sepia/env`;

/**
 * The stock env template — written once, never overwritten. Comments list
 * every knob so editing doesn't need docs in the unit.
 */
export const envTemplate =
  (): string => `# sepia service environment — KEY=value per line, no quoting.
# See DEPLOY.md "SEPIA_* environment" for the full reference.

# Bearer auth on every /api/* route (required for non-loopback binds).
# SEPIA_TOKEN=
# Bind address — non-loopback requires SEPIA_TOKEN.
# SEPIA_HOST=127.0.0.1
# Devin store path (opened read-only).
# SEPIA_DB=$HOME/.local/share/devin/cli/sessions.db
# Overlay session stores.
# SEPIA_CLINE_DIR=$HOME/.cline/data
# SEPIA_CLAUDE_DIR=$HOME/.claude
# SEPIA_CURSOR_DIR=$HOME/.cursor
`;

/** Detect the backend for the current platform; null = unsupported. */
export const detectBackend = async (
  platform: NodeJS.Platform = process.platform,
): Promise<ServiceBackend | null> => {
  if (platform === "linux") {
    const mod = await import("./systemd.js");
    return mod.systemdBackend;
  }
  if (platform === "darwin") {
    const mod = await import("./launchd.js");
    return mod.launchdBackend;
  }
  return null;
};
